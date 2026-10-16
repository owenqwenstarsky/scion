use scion_core::{BlobId, Change, Changeset, ChangesetId, FileKind, Identity, RepoPath, Tree};
use scion_store::{Cas, Store};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;

fn ident() -> Identity {
    Identity::new("ada", "ada@example.com", 1_700_000_000)
}

fn changeset(parents: Vec<ChangesetId>, path: &str, bytes: &[u8]) -> (Changeset, BlobId) {
    let id = BlobId::of(bytes);
    let mut changes = BTreeMap::new();
    changes.insert(
        RepoPath::new(path).unwrap(),
        Change::Put {
            kind: FileKind::Regular,
            blob: id,
            size: bytes.len() as u64,
        },
    );
    let cs = Changeset::create(parents, changes, ident(), ident(), "m", BTreeMap::new());
    (cs, id)
}

#[test]
fn blob_roundtrip_small_and_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let small = b"hello scion";
    let id = BlobId::of(small);
    store.put_blob(id, small).unwrap();
    store.put_blob(id, small).unwrap();
    assert_eq!(store.get_blob(id).unwrap(), small);

    let medium = vec![b'a'; 40_000];
    let mid = BlobId::of(&medium);
    store.put_blob(mid, &medium).unwrap();
    assert_eq!(store.get_blob(mid).unwrap(), medium);
    assert!(store.put_blob(mid, b"nope").is_err());
}

#[test]
fn chunk_dedup_across_versions() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut a = vec![0u8; 1_200_000];
    for (i, b) in a.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    let mut b = a.clone();
    let last = b.len() - 1;
    b[last] ^= 0xff;
    let id_a = BlobId::of(&a);
    let id_b = BlobId::of(&b);
    store.put_blob(id_a, &a).unwrap();
    let before = store.blob_count().unwrap();
    store.put_blob(id_b, &b).unwrap();
    let after = store.blob_count().unwrap();
    assert_eq!(store.get_blob(id_a).unwrap(), a);
    assert_eq!(store.get_blob(id_b).unwrap(), b);
    // The second file reuses earlier chunks, so the index grows by less than a full extra copy.
    assert!(after - before < 20, "before {before} after {after}");
    let range = store.get_blob_range(id_a, Some(0..4)).unwrap();
    assert_eq!(range, a[..4]);
}

#[test]
fn changeset_idempotent_and_cas_single_winner() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (cs, blob) = changeset(vec![], "README.md", b"v1");
    store.put_blob(blob, b"v1").unwrap();
    assert!(store.put_changeset("ns", &cs).unwrap());
    assert!(!store.put_changeset("ns", &cs).unwrap());
    assert_eq!(store.get_changeset("ns", cs.id).unwrap().unwrap().id, cs.id);

    let (a, ba) = changeset(vec![cs.id], "README.md", b"left");
    let (b, bb) = changeset(vec![cs.id], "README.md", b"right");
    store.put_blob(ba, b"left").unwrap();
    store.put_blob(bb, b"right").unwrap();
    store.put_changeset("ns", &a).unwrap();
    store.put_changeset("ns", &b).unwrap();

    let first = Cas {
        ns: "ns".into(),
        name: "main".into(),
        expected: None,
        new_target: cs.id,
        source: cs.id,
        actor: "t".into(),
    };
    assert!(store.cas_ref(&first).unwrap());
    assert!(!store.cas_ref(&first).unwrap());

    let store = Arc::new(store);
    let mut handles = Vec::new();
    for (cand, actor) in [(a.id, "a"), (b.id, "b")] {
        let store = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            store.cas_ref(&Cas {
                ns: "ns".into(),
                name: "main".into(),
                expected: Some(cs.id),
                new_target: cand,
                source: cand,
                actor: actor.into(),
            })
        }));
    }
    let results: Vec<bool> = handles
        .into_iter()
        .map(|h| h.join().unwrap().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|w| **w).count(), 1, "{results:?}");
    let tip = store.get_ref("ns", "main").unwrap().unwrap();
    assert!(tip.target == a.id || tip.target == b.id);
    assert!(store.when_landed("ns", tip.target).unwrap().is_some());
    let doc = store.doctor().unwrap();
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);
}

#[test]
fn tree_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let tree = Tree::empty();
    let id = store.put_tree(&tree).unwrap();
    assert_eq!(store.get_tree(id).unwrap(), tree);
}

#[test]
fn reopen_survives() {
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let store = Store::open(dir.path()).unwrap();
        let raw = b"persist";
        let id = BlobId::of(raw);
        store.put_blob(id, raw).unwrap();
        id
    };
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.get_blob(id).unwrap(), b"persist");
    assert_eq!(store.schema_version().unwrap(), "1");
}
