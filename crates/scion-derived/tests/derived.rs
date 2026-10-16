use scion_core::{BlobId, Change, Changeset, FileKind, Identity, RepoPath};
use scion_derived::Engine;
use scion_store::Store;
use std::collections::BTreeMap;
use std::sync::Arc;

fn ident() -> Identity {
    Identity::new("ada", "ada@example.com", 10)
}

fn commit(
    store: &Store,
    ns: &str,
    parents: Vec<scion_core::ChangesetId>,
    files: &[(&str, &[u8])],
) -> scion_core::ChangesetId {
    let mut changes = BTreeMap::new();
    for (path, bytes) in files {
        let id = BlobId::of(bytes);
        store.put_blob(id, bytes).unwrap();
        changes.insert(
            RepoPath::new(*path).unwrap(),
            Change::Put {
                kind: FileKind::Regular,
                blob: id,
                size: bytes.len() as u64,
            },
        );
    }
    let cs = Changeset::create(parents, changes, ident(), ident(), "m", BTreeMap::new());
    store.put_changeset(ns, &cs).unwrap();
    cs.id
}

#[test]
fn manifest_blame_and_graph() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let engine = Engine::new(Arc::clone(&store));
    let c1 = commit(&store, "r", vec![], &[("a.txt", b"one\ntwo\nthree\n")]);
    let c2 = commit(
        &store,
        "r",
        vec![c1],
        &[("a.txt", b"one\nTWO\nthree\n"), ("b.txt", b"new\n")],
    );
    let root = engine.manifest("r", c2).unwrap();
    let again = engine.manifest("r", c2).unwrap();
    assert_eq!(root, again);
    let entries = engine.list_dir("r", c2, None).unwrap();
    assert_eq!(entries.len(), 2);
    let blame = engine
        .blame("r", c2, &RepoPath::new("a.txt").unwrap())
        .unwrap();
    assert!(!blame.binary);
    assert!(blame.ranges.iter().any(|r| r.origin == c2));
    assert!(blame.ranges.iter().any(|r| r.origin == c1));
    let g2 = engine.graph("r", c2).unwrap();
    let g1 = engine.graph("r", c1).unwrap();
    assert!(g2.generation > g1.generation);
    assert!(engine.is_ancestor("r", c1, c2).unwrap());
    assert!(!engine.is_ancestor("r", c2, c1).unwrap());
    assert_eq!(engine.merge_base("r", c1, c2).unwrap(), Some(c1));
    let hist = engine
        .path_history("r", &RepoPath::new("a.txt").unwrap(), 10)
        .unwrap();
    assert_eq!(hist.len(), 2);
    let diff = engine.diff("r", Some(c1), c2).unwrap();
    assert!(diff
        .iter()
        .any(|d| d.path == "b.txt" && d.status == "added"));
}

#[test]
fn eager_matches_recompute() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let engine = Engine::new(Arc::clone(&store));
    let c1 = commit(&store, "r", vec![], &[("f", b"a\n")]);
    engine.ensure_eager("r", c1).unwrap();
    let cached = engine.blame("r", c1, &RepoPath::new("f").unwrap()).unwrap();
    // Second call hits the derived row and must match.
    let again = engine.blame("r", c1, &RepoPath::new("f").unwrap()).unwrap();
    assert_eq!(cached, again);
}
