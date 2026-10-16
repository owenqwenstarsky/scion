use proptest::prelude::*;
use scion_core::{
    apply_changes, changes_between, BlobId, Change, Changeset, ChangesetId, FileKind, Identity,
    RepoPath, Tree, TreeEntry,
};
use std::collections::BTreeMap;

fn identity() -> Identity {
    Identity::new("ada", "ada@example.com", 1_700_000_000)
}

fn blob(n: u8) -> BlobId {
    BlobId::of(&[n])
}

fn put(n: u8) -> Change {
    Change::Put {
        kind: FileKind::Regular,
        blob: blob(n),
        size: 1,
    }
}

#[test]
fn changeset_roundtrip() {
    let mut changes = BTreeMap::new();
    changes.insert(RepoPath::new("src/main.rs").unwrap(), put(1));
    changes.insert(RepoPath::new("README.md").unwrap(), Change::Delete);
    let mut extra = BTreeMap::new();
    extra.insert("git.oid".into(), b"abc".to_vec());
    let parent = ChangesetId::of(b"parent");
    let cs = Changeset::create(
        vec![parent],
        changes,
        identity(),
        identity(),
        "hello",
        extra,
    );
    let bytes = cs.encode();
    let decoded = Changeset::decode(&bytes).unwrap();
    assert_eq!(decoded, cs);
    assert_eq!(Changeset::decode(&decoded.encode()).unwrap().id, cs.id);
}

#[test]
fn tree_roundtrip_and_stable_empty() {
    let mut entries = BTreeMap::new();
    entries.insert(
        "a".into(),
        TreeEntry::File {
            kind: FileKind::Executable,
            blob: blob(7),
            size: 3,
        },
    );
    let nested = Tree::empty();
    entries.insert("d".into(), TreeEntry::Dir { tree: nested.id() });
    let tree = Tree { entries };
    let decoded = Tree::decode(&tree.encode()).unwrap();
    assert_eq!(decoded, tree);
    assert_eq!(
        Tree::empty().id(),
        Tree::decode(&Tree::empty().encode()).unwrap().id()
    );
}

#[test]
fn apply_then_diff_is_identity() {
    let mut store: BTreeMap<scion_core::TreeId, Tree> = BTreeMap::new();
    let mut save = |t: &Tree| {
        let id = t.id();
        store.insert(id, t.clone());
        Ok(id)
    };
    let mut changes = BTreeMap::new();
    changes.insert(RepoPath::new("a/b.txt").unwrap(), put(1));
    changes.insert(RepoPath::new("c.txt").unwrap(), put(2));
    let root = apply_changes(
        None,
        &changes,
        |_| Err(scion_core::Error::NotFound),
        &mut save,
    )
    .unwrap();
    let back = changes_between(None, root, |id| {
        store.get(&id).cloned().ok_or(scion_core::Error::NotFound)
    })
    .unwrap();
    assert_eq!(back, changes);
}

#[test]
fn golden_vectors_are_stable() {
    // Frozen after the first implementation. Changing the encoding requires a
    // domain-string version bump (`scion tree v2`, `scion changeset v2`).
    let empty = Tree::empty();
    let cs = Changeset::create(
        vec![],
        BTreeMap::new(),
        identity(),
        identity(),
        "",
        BTreeMap::new(),
    );
    let tree_hex = empty.id().to_hex();
    let cs_hex = cs.id.to_hex();
    let expected_tree = include_str!("vectors/empty_tree.id").trim();
    let expected_cs = include_str!("vectors/empty_changeset.id").trim();
    assert_eq!(tree_hex, expected_tree, "tree encoding changed");
    assert_eq!(cs_hex, expected_cs, "changeset encoding changed");
}

proptest! {
    #[test]
    fn encoding_is_deterministic(message in "[a-z]{0,40}", n in 0u8..20) {
        let mut changes = BTreeMap::new();
        for i in 0..n {
            changes.insert(RepoPath::new(format!("f{i}")).unwrap(), put(i));
        }
        let cs = Changeset::create(vec![], changes, identity(), identity(), message, BTreeMap::new());
        let a = cs.encode();
        let b = Changeset::decode(&a).unwrap().encode();
        prop_assert_eq!(a, b);
    }

    #[test]
    fn path_roundtrip(s in "[a-z][a-z0-9]{0,8}(/[a-z][a-z0-9]{0,8}){0,3}") {
        let p = RepoPath::new(&s).unwrap();
        prop_assert_eq!(RepoPath::new(p.as_str()).unwrap(), p);
    }
}
