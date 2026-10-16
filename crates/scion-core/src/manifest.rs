//! Path-copying manifest edits. Trees are addressed by [`TreeId`]; the caller
//! loads and stores canonical trees so this module stays free of I/O.

use crate::error::{Error, Result};
use crate::id::TreeId;
use crate::model::{Change, Tree, TreeEntry};
use crate::path::RepoPath;
use std::collections::BTreeMap;

pub type TreeDiff = BTreeMap<RepoPath, (Option<TreeEntry>, Option<TreeEntry>)>;

pub fn empty_tree() -> Tree {
    Tree::empty()
}

/// Apply `changes` (relative to `root`, or an empty tree) and return the new root id.
///
/// Directory nodes that become empty are removed, except the root, which is
/// always stored so clients have a stable starting tree.
pub fn apply_changes(
    root: Option<TreeId>,
    changes: &BTreeMap<RepoPath, Change>,
    mut load: impl FnMut(TreeId) -> Result<Tree>,
    mut store: impl FnMut(&Tree) -> Result<TreeId>,
) -> Result<TreeId> {
    let tree = match root {
        Some(id) => load(id)?,
        None => Tree::empty(),
    };
    let flat: Vec<(Vec<String>, &Change)> = changes
        .iter()
        .map(|(p, c)| (p.segments().map(str::to_string).collect(), c))
        .collect();
    apply_dir(&tree, &flat, true, &mut load, &mut store)?
        .ok_or_else(|| Error::Other("root tree missing".into()))
}

fn apply_dir(
    tree: &Tree,
    changes: &[(Vec<String>, &Change)],
    is_root: bool,
    load: &mut impl FnMut(TreeId) -> Result<Tree>,
    store: &mut impl FnMut(&Tree) -> Result<TreeId>,
) -> Result<Option<TreeId>> {
    let mut file_ops: BTreeMap<String, &Change> = BTreeMap::new();
    let mut dir_ops: BTreeMap<String, Vec<(Vec<String>, &Change)>> = BTreeMap::new();
    for (segs, change) in changes {
        if segs.is_empty() {
            return Err(Error::Invalid(
                "empty path segment while applying changes".into(),
            ));
        }
        if segs.len() == 1 {
            file_ops.insert(segs[0].clone(), *change);
        } else {
            dir_ops
                .entry(segs[0].clone())
                .or_default()
                .push((segs[1..].to_vec(), *change));
        }
    }

    let mut entries = tree.entries.clone();
    for (name, change) in &file_ops {
        match change {
            Change::Delete => {
                entries.remove(name);
            }
            Change::Put { kind, blob, size } => {
                entries.insert(
                    name.clone(),
                    TreeEntry::File {
                        kind: *kind,
                        blob: *blob,
                        size: *size,
                    },
                );
            }
        }
    }

    for (name, sub) in &dir_ops {
        if file_ops.contains_key(name) {
            // A file put/delete at this name wins over nested edits; nested
            // edits under a path that is itself deleted or replaced are dropped
            // only when the file op is Delete of the directory's replacement.
            // Callers (merge) reject directory/file conflicts before this point.
            // If both exist, the file op already replaced the entry; skip nesting.
            continue;
        }
        let child = match entries.get(name) {
            Some(TreeEntry::Dir { tree }) => load(*tree)?,
            Some(TreeEntry::File { .. }) | None => Tree::empty(),
        };
        match apply_dir(&child, sub, false, load, store)? {
            Some(id) => {
                entries.insert(name.clone(), TreeEntry::Dir { tree: id });
            }
            None => {
                entries.remove(name);
            }
        }
    }

    let built = Tree { entries };
    if built.entries.is_empty() && !is_root {
        return Ok(None);
    }
    Ok(Some(store(&built)?))
}

pub fn lookup_entry(
    root: TreeId,
    path: &RepoPath,
    mut load: impl FnMut(TreeId) -> Result<Tree>,
) -> Result<Option<TreeEntry>> {
    let mut current = load(root)?;
    let segs: Vec<&str> = path.segments().collect();
    for (i, seg) in segs.iter().enumerate() {
        match current.entries.get(*seg) {
            None => return Ok(None),
            Some(TreeEntry::Dir { tree }) => {
                if i + 1 == segs.len() {
                    return Ok(Some(TreeEntry::Dir { tree: *tree }));
                }
                current = load(*tree)?;
            }
            Some(entry @ TreeEntry::File { .. }) => {
                if i + 1 == segs.len() {
                    return Ok(Some(entry.clone()));
                }
                return Ok(None);
            }
        }
    }
    Ok(None)
}

/// Diff two trees. The result maps path → (left, right). `None` means absent.
pub fn diff_trees(
    left: TreeId,
    right: TreeId,
    mut load: impl FnMut(TreeId) -> Result<Tree>,
) -> Result<TreeDiff> {
    let mut out = BTreeMap::new();
    diff_rec("", left, right, &mut load, &mut out)?;
    Ok(out)
}

fn diff_rec(
    prefix: &str,
    left: TreeId,
    right: TreeId,
    load: &mut impl FnMut(TreeId) -> Result<Tree>,
    out: &mut TreeDiff,
) -> Result<()> {
    if left == right {
        return Ok(());
    }
    let a = load(left)?;
    let b = load(right)?;
    let mut names: BTreeMap<&str, ()> = BTreeMap::new();
    for n in a.entries.keys() {
        names.insert(n, ());
    }
    for n in b.entries.keys() {
        names.insert(n, ());
    }
    for name in names.keys() {
        let path = if prefix.is_empty() {
            (*name).to_string()
        } else {
            format!("{prefix}/{name}")
        };
        match (a.entries.get(*name), b.entries.get(*name)) {
            (Some(l), Some(r)) if l == r => {}
            (Some(TreeEntry::Dir { tree: lt }), Some(TreeEntry::Dir { tree: rt })) => {
                diff_rec(&path, *lt, *rt, load, out)?;
            }
            (l, r) => {
                let repo_path = RepoPath::new(&path)?;
                let file_side = |e: Option<&TreeEntry>| match e {
                    Some(TreeEntry::File { .. }) => e.cloned(),
                    Some(TreeEntry::Dir { .. }) => e.cloned(),
                    None => None,
                };
                // Expand a directory that disappeared into per-file deletions/additions
                // only when one side is a directory and the other is not.
                match (l, r) {
                    (Some(TreeEntry::Dir { tree }), other) => {
                        flatten_dir(&path, *tree, true, load, out)?;
                        if other.is_some() {
                            out.insert(repo_path, (None, file_side(other)));
                        }
                    }
                    (other, Some(TreeEntry::Dir { tree })) => {
                        if other.is_some() {
                            out.insert(RepoPath::new(&path)?, (file_side(other), None));
                        }
                        flatten_dir(&path, *tree, false, load, out)?;
                    }
                    _ => {
                        out.insert(repo_path, (file_side(l), file_side(r)));
                    }
                }
            }
        }
    }
    Ok(())
}

fn flatten_dir(
    prefix: &str,
    tree: TreeId,
    left_side: bool,
    load: &mut impl FnMut(TreeId) -> Result<Tree>,
    out: &mut TreeDiff,
) -> Result<()> {
    let t = load(tree)?;
    for (name, entry) in &t.entries {
        let path = format!("{prefix}/{name}");
        match entry {
            TreeEntry::Dir { tree } => flatten_dir(&path, *tree, left_side, load, out)?,
            file => {
                let rp = RepoPath::new(&path)?;
                if left_side {
                    out.insert(rp, (Some(file.clone()), None));
                } else {
                    out.insert(rp, (None, Some(file.clone())));
                }
            }
        }
    }
    Ok(())
}

/// Changes that turn `parent` into `child` (file-level, relative to parent).
pub fn changes_between(
    parent: Option<TreeId>,
    child: TreeId,
    load: impl FnMut(TreeId) -> Result<Tree>,
) -> Result<BTreeMap<RepoPath, Change>> {
    let mut load = load;
    let empty = empty_tree();
    let empty_id = empty.id();
    // The empty tree may not be stored. Handle parent=None directly.
    if parent.is_none() {
        let mut out = BTreeMap::new();
        flatten_adds(child, "", &mut load, &mut out)?;
        return Ok(out);
    }
    let parent = parent.unwrap();
    let _ = empty_id;
    let diff = diff_trees(parent, child, load)?;
    let mut changes = BTreeMap::new();
    for (path, (l, r)) in diff {
        match (l, r) {
            (Some(TreeEntry::File { .. }), Some(TreeEntry::File { kind, blob, size })) => {
                changes.insert(path, Change::Put { kind, blob, size });
            }
            (None, Some(TreeEntry::File { kind, blob, size })) => {
                changes.insert(path, Change::Put { kind, blob, size });
            }
            (Some(TreeEntry::File { .. }), None) => {
                changes.insert(path, Change::Delete);
            }
            (Some(TreeEntry::Dir { .. }), None) => {
                changes.insert(path, Change::Delete);
            }
            (None, Some(TreeEntry::Dir { .. })) => {
                // A directory appearing without files is not representable as a file change.
            }
            (Some(TreeEntry::Dir { .. }), Some(TreeEntry::File { kind, blob, size })) => {
                changes.insert(path, Change::Put { kind, blob, size });
            }
            (Some(TreeEntry::File { .. }), Some(TreeEntry::Dir { .. })) => {
                changes.insert(path, Change::Delete);
            }
            (None, None) | (Some(TreeEntry::Dir { .. }), Some(TreeEntry::Dir { .. })) => {}
        }
    }
    Ok(changes)
}

fn flatten_adds(
    tree: TreeId,
    prefix: &str,
    load: &mut impl FnMut(TreeId) -> Result<Tree>,
    out: &mut BTreeMap<RepoPath, Change>,
) -> Result<()> {
    let t = load(tree)?;
    for (name, entry) in t.entries {
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        match entry {
            TreeEntry::Dir { tree } => flatten_adds(tree, &path, load, out)?,
            TreeEntry::File { kind, blob, size } => {
                out.insert(RepoPath::new(&path)?, Change::Put { kind, blob, size });
            }
        }
    }
    Ok(())
}
