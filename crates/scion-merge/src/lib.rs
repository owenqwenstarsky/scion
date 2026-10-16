#![allow(clippy::result_large_err)]
//! Pure rebase/cherry-pick of a changeset's path delta onto another tree.
//! Callers supply the file state at the base and at the destination tip.

use scion_core::{BlobId, Change, FileKind, RepoPath};
use similar::{DiffOp, TextDiff};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeState {
    Absent,
    File {
        kind: FileKind,
        blob: BlobId,
        size: u64,
    },
    Dir,
}

impl NodeState {
    pub fn blob(&self) -> Option<BlobId> {
        match self {
            NodeState::File { blob, .. } => Some(*blob),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub path: RepoPath,
    pub kind: &'static str,
    pub base: Option<BlobId>,
    pub ours: Option<BlobId>,
    pub theirs: Option<BlobId>,
    /// Merged text with conflict markers, when a text merge was attempted.
    pub merged: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("{count} conflict(s)")]
pub struct Conflicts {
    pub count: usize,
    pub items: Vec<Conflict>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathAction {
    Keep(Change),
    Drop,
    Merged { kind: FileKind, text: String },
}

fn same(a: &NodeState, b: &NodeState) -> bool {
    a == b
}

fn applied(change: &Change) -> NodeState {
    match change {
        Change::Delete => NodeState::Absent,
        Change::Put { kind, blob, size } => NodeState::File {
            kind: *kind,
            blob: *blob,
            size: *size,
        },
    }
}

/// Integrate one path. `read_text` returns UTF-8 text for a blob, or `None` when
/// the bytes are binary.
#[allow(clippy::result_large_err)]
pub fn integrate_path(
    path: &RepoPath,
    base: &NodeState,
    ours: &NodeState,
    change: &Change,
    mut read_text: impl FnMut(BlobId) -> Option<String>,
) -> Result<PathAction, Conflict> {
    let applied = applied(change);
    if same(ours, base) {
        return Ok(PathAction::Keep(change.clone()));
    }
    if same(ours, &applied) {
        return Ok(PathAction::Drop);
    }
    match (base, ours, change) {
        (
            NodeState::File { .. } | NodeState::Absent,
            NodeState::File {
                kind: ours_kind, ..
            },
            Change::Put {
                kind: theirs_kind, ..
            },
        ) if matches!(ours_kind, FileKind::Regular | FileKind::Executable)
            && matches!(theirs_kind, FileKind::Regular | FileKind::Executable) =>
        {
            let base_text = match base {
                NodeState::File { blob, .. } => read_text(*blob),
                NodeState::Absent => Some(String::new()),
                NodeState::Dir => None,
            };
            let ours_text = ours.blob().and_then(&mut read_text);
            let theirs_text = applied.blob().and_then(&mut read_text);
            if let (Some(b), Some(o), Some(t)) = (base_text, ours_text, theirs_text) {
                match merge_lines(&b, &o, &t) {
                    Ok(text) => {
                        return Ok(PathAction::Merged {
                            kind: *theirs_kind,
                            text,
                        })
                    }
                    Err(marked) => {
                        return Err(Conflict {
                            path: path.clone(),
                            kind: "modify/modify",
                            base: base.blob(),
                            ours: ours.blob(),
                            theirs: applied.blob(),
                            merged: Some(marked),
                        })
                    }
                }
            }
            Err(conflict(path, "modify/modify", base, ours, &applied))
        }
        (NodeState::Dir, _, Change::Put { .. }) | (_, NodeState::Dir, Change::Put { .. }) => {
            Err(conflict(path, "file/dir", base, ours, &applied))
        }
        (_, NodeState::Absent, Change::Put { .. }) => {
            Err(conflict(path, "modify/delete", base, ours, &applied))
        }
        (_, NodeState::File { .. }, Change::Delete) => {
            Err(conflict(path, "modify/delete", base, ours, &applied))
        }
        _ => Err(conflict(path, "conflict", base, ours, &applied)),
    }
}

fn conflict(
    path: &RepoPath,
    kind: &'static str,
    base: &NodeState,
    ours: &NodeState,
    theirs: &NodeState,
) -> Conflict {
    Conflict {
        path: path.clone(),
        kind,
        base: base.blob(),
        ours: ours.blob(),
        theirs: theirs.blob(),
        merged: None,
    }
}

struct Region {
    start: usize,
    end: usize,
    lines: Vec<String>,
}

fn split_keep(s: &str) -> Vec<&str> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.split_inclusive('\n').collect()
    }
}

fn regions(base: &str, other: &str) -> Vec<Region> {
    let other_lines = split_keep(other);
    let diff = TextDiff::from_lines(base, other);
    let mut out = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index, old_len, ..
            } => out.push(Region {
                start: old_index,
                end: old_index + old_len,
                lines: Vec::new(),
            }),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => out.push(Region {
                start: old_index,
                end: old_index,
                lines: other_lines[new_index..new_index + new_len]
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            }),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => out.push(Region {
                start: old_index,
                end: old_index + old_len,
                lines: other_lines[new_index..new_index + new_len]
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            }),
        }
    }
    out
}

/// Line-level 3-way merge. Overlapping edits to the same base lines conflict;
/// edits to different lines combine.
pub fn merge_lines(base: &str, ours: &str, theirs: &str) -> Result<String, String> {
    let base_lines = split_keep(base);
    let o_regions = regions(base, ours);
    let t_regions = regions(base, theirs);
    let mut out = String::new();
    let mut i = 0;
    let mut oi = 0;
    let mut ti = 0;
    let n = base_lines.len();
    while i <= n {
        let o = o_regions.get(oi).filter(|r| r.start == i);
        let t = t_regions.get(ti).filter(|r| r.start == i);
        match (o, t) {
            (None, None) => {
                if i >= n {
                    break;
                }
                out.push_str(base_lines[i]);
                i += 1;
            }
            (Some(r), None) => {
                for line in &r.lines {
                    out.push_str(line);
                }
                i = r.end.max(i);
                if r.end == r.start {
                    // pure insert: stay on this base index but consume the region
                }
                oi += 1;
                if r.end == r.start && i == r.start {
                    // avoid spinning if a zero-width region is the only one left and i does not move
                }
            }
            (None, Some(r)) => {
                for line in &r.lines {
                    out.push_str(line);
                }
                i = if r.end == r.start { i } else { r.end };
                ti += 1;
            }
            (Some(ro), Some(rt)) => {
                if ro.end == rt.end && ro.lines == rt.lines {
                    for line in &ro.lines {
                        out.push_str(line);
                    }
                    i = if ro.end == ro.start { i } else { ro.end };
                    oi += 1;
                    ti += 1;
                } else {
                    let mut marked = String::new();
                    marked.push_str("<<<<<<< ours\n");
                    for line in &ro.lines {
                        marked.push_str(line);
                        if !line.ends_with('\n') {
                            marked.push('\n');
                        }
                    }
                    marked.push_str("=======\n");
                    for line in &rt.lines {
                        marked.push_str(line);
                        if !line.ends_with('\n') {
                            marked.push('\n');
                        }
                    }
                    marked.push_str(">>>>>>> theirs\n");
                    return Err(marked);
                }
            }
        }
        if oi > o_regions.len() + 2 && ti > t_regions.len() + 2 {
            break;
        }
    }
    Ok(out)
}

/// Rebase `changes` (a delta against `base_at`) onto `ours_at`.
pub fn rebase_changes(
    changes: &BTreeMap<RepoPath, Change>,
    mut base_at: impl FnMut(&RepoPath) -> NodeState,
    mut ours_at: impl FnMut(&RepoPath) -> NodeState,
    mut read_text: impl FnMut(BlobId) -> Option<String>,
) -> Result<Vec<(RepoPath, PathAction)>, Conflicts> {
    let mut actions = Vec::new();
    let mut conflicts = Vec::new();
    for (path, change) in changes {
        let base = base_at(path);
        let ours = ours_at(path);
        match integrate_path(path, &base, &ours, change, &mut read_text) {
            Ok(action) => actions.push((path.clone(), action)),
            Err(c) => conflicts.push(c),
        }
    }
    if conflicts.is_empty() {
        Ok(actions)
    } else {
        Err(Conflicts {
            count: conflicts.len(),
            items: conflicts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scion_core::BlobId;

    fn file(bytes: &[u8]) -> (NodeState, BlobId) {
        let id = BlobId::of(bytes);
        (
            NodeState::File {
                kind: FileKind::Regular,
                blob: id,
                size: bytes.len() as u64,
            },
            id,
        )
    }

    #[test]
    fn text_merge_properties() {
        let base = "a\nb\nc\n";
        let ours = "a\nB\nc\n";
        let theirs = "a\nb\nC\n";
        let merged = merge_lines(base, ours, theirs).unwrap();
        assert_eq!(merged, "a\nB\nC\n");
        assert_eq!(merge_lines(base, ours, ours).unwrap(), ours);
        assert_eq!(merge_lines(base, base, theirs).unwrap(), theirs);
        assert_eq!(merge_lines(base, ours, base).unwrap(), ours);
        assert!(merge_lines(base, "A\nb\nc\n", "Z\nb\nc\n").is_err());
    }

    #[test]
    fn disjoint_keep_and_convergent_drop() {
        let (base, _) = file(b"base\n");
        let change = Change::Put {
            kind: FileKind::Regular,
            blob: BlobId::of(b"next\n"),
            size: 5,
        };
        let path = RepoPath::new("f").unwrap();
        let action = integrate_path(&path, &base, &base, &change, |_| Some("x".into())).unwrap();
        assert!(matches!(action, PathAction::Keep(_)));

        let applied = NodeState::File {
            kind: FileKind::Regular,
            blob: BlobId::of(b"next\n"),
            size: 5,
        };
        let action = integrate_path(&path, &base, &applied, &change, |_| None).unwrap();
        assert!(matches!(action, PathAction::Drop));
    }

    #[test]
    fn overlapping_text_merges() {
        let (base, base_id) = file(b"a\nb\nc\n");
        let (ours, ours_id) = file(b"a\nB\nc\n");
        let (theirs_state, theirs_id) = file(b"a\nb\nC\n");
        let change = Change::Put {
            kind: FileKind::Regular,
            blob: match &theirs_state {
                NodeState::File { blob, .. } => *blob,
                _ => unreachable!(),
            },
            size: 7,
        };
        let path = RepoPath::new("f").unwrap();
        let texts = |id: BlobId| {
            if id == base_id {
                Some("a\nb\nc\n".into())
            } else if id == ours_id {
                Some("a\nB\nc\n".into())
            } else if id == theirs_id {
                Some("a\nb\nC\n".into())
            } else {
                None
            }
        };
        let action = integrate_path(&path, &base, &ours, &change, texts).unwrap();
        match action {
            PathAction::Merged { text, .. } => assert_eq!(text, "a\nB\nC\n"),
            other => panic!("{other:?}"),
        }
    }
}
