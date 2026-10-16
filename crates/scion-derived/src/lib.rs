//! Derived data computed from changesets and written back to the metadata store.
//!
//! Kinds are defined from parent values. Eager kinds (manifest, commit graph,
//! changed paths) are computed before a ref compare-and-swap. Blame and the
//! path log are lazy and match an eager recomputation.

use scion_core::{
    apply_changes, changes_between, lookup_entry, Change, Changeset, ChangesetId, RepoPath, Tree,
    TreeEntry, TreeId,
};
use scion_merge::NodeState;
use scion_store::Store;
use similar::{ChangeTag, TextDiff};
use std::sync::Arc;

const MANIFEST: &str = "manifest";
const GRAPH: &str = "commit_graph";
const CHANGED: &str = "changed_paths";
const PATH_LOG: &str = "path_log";
const BLAME: &str = "blame";
const VER: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphValue {
    pub generation: u64,
    pub skips: Vec<ChangesetId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameRange {
    pub start_line: u32,
    pub count: u32,
    pub origin: ChangesetId,
    pub origin_start: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blame {
    pub binary: bool,
    pub ranges: Vec<BlameRange>,
}

#[derive(Clone, Debug)]
pub struct DiffFile {
    pub path: String,
    pub status: &'static str,
    pub patch: String,
}

pub struct Engine {
    store: Arc<Store>,
}

impl Engine {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn manifest(&self, ns: &str, id: ChangesetId) -> scion_store::Result<TreeId> {
        if let Some(hit) = self.load_manifest(ns, id)? {
            return Ok(hit);
        }
        let mut pending = Vec::new();
        let mut cursor = Some(id);
        let mut base: Option<TreeId> = None;
        while let Some(cur) = cursor {
            if let Some(hit) = self.load_manifest(ns, cur)? {
                base = Some(hit);
                break;
            }
            let cs = self
                .store
                .get_changeset(ns, cur)?
                .ok_or(scion_core::Error::NotFound)?;
            cursor = cs.first_parent();
            pending.push(cs);
        }
        pending.reverse();
        let mut parent = base;
        let mut last = base.ok_or(scion_core::Error::NotFound);
        for cs in pending {
            let tid = self.materialize(&cs, parent)?;
            self.store_manifest(ns, cs.id, tid)?;
            self.note_paths(ns, &cs)?;
            self.store_graph(ns, &cs)?;
            self.store_changed(ns, &cs)?;
            parent = Some(tid);
            last = Ok(tid);
        }
        last.map_err(scion_store::Error::from)
    }

    pub fn ensure_eager(&self, ns: &str, id: ChangesetId) -> scion_store::Result<TreeId> {
        self.manifest(ns, id)
    }

    pub fn node_at(
        &self,
        ns: &str,
        cs: ChangesetId,
        path: &RepoPath,
    ) -> scion_store::Result<NodeState> {
        let root = self.manifest(ns, cs)?;
        let entry = lookup_entry(root, path, |id| self.load_tree(id))?;
        Ok(match entry {
            None => NodeState::Absent,
            Some(TreeEntry::Dir { .. }) => NodeState::Dir,
            Some(TreeEntry::File { kind, blob, size }) => NodeState::File { kind, blob, size },
        })
    }

    pub fn read_file(
        &self,
        ns: &str,
        cs: ChangesetId,
        path: &RepoPath,
    ) -> scion_store::Result<Option<Vec<u8>>> {
        match self.node_at(ns, cs, path)? {
            NodeState::File { blob, .. } => Ok(Some(self.store.get_blob(blob)?)),
            _ => Ok(None),
        }
    }

    pub fn list_dir(
        &self,
        ns: &str,
        cs: ChangesetId,
        path: Option<&RepoPath>,
    ) -> scion_store::Result<Vec<(String, TreeEntry)>> {
        let root = self.manifest(ns, cs)?;
        let tree = if let Some(path) = path {
            match lookup_entry(root, path, |id| self.load_tree(id))? {
                Some(TreeEntry::Dir { tree }) => self.load_tree(tree)?,
                Some(_) => return Err(scion_core::Error::Invalid("not a directory".into()).into()),
                None => return Err(scion_core::Error::NotFound.into()),
            }
        } else {
            self.load_tree(root)?
        };
        Ok(tree.entries.into_iter().collect())
    }

    pub fn graph(&self, ns: &str, id: ChangesetId) -> scion_store::Result<GraphValue> {
        if let Some(g) = self.load_graph(ns, id)? {
            return Ok(g);
        }
        let cs = self
            .store
            .get_changeset(ns, id)?
            .ok_or(scion_core::Error::NotFound)?;
        // Manifest walk also stores the graph; force it so parents exist.
        self.manifest(ns, id)?;
        self.load_graph(ns, id)?
            .ok_or_else(|| scion_store::Error::Msg(format!("graph missing for {}", cs.id)))
    }

    pub fn is_ancestor(
        &self,
        ns: &str,
        ancestor: ChangesetId,
        descendant: ChangesetId,
    ) -> scion_store::Result<bool> {
        if ancestor == descendant {
            return Ok(true);
        }
        let target = self.graph(ns, ancestor)?.generation;
        let mut cur = descendant;
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 1_000_000 {
                return Ok(false);
            }
            if cur == ancestor {
                return Ok(true);
            }
            let g = self.graph(ns, cur)?;
            if g.generation <= target {
                return Ok(false);
            }
            let mut next = None;
            for skip in g.skips.iter().rev() {
                let sg = self.graph(ns, *skip)?;
                if sg.generation >= target {
                    next = Some(*skip);
                    break;
                }
            }
            match next {
                Some(n) if n != cur => cur = n,
                _ => return Ok(false),
            }
        }
    }

    pub fn merge_base(
        &self,
        ns: &str,
        a: ChangesetId,
        b: ChangesetId,
    ) -> scion_store::Result<Option<ChangesetId>> {
        if a == b {
            return Ok(Some(a));
        }
        let mut aset = std::collections::BTreeSet::new();
        let mut stack = vec![a];
        while let Some(c) = stack.pop() {
            if !aset.insert(c) {
                continue;
            }
            if let Some(cs) = self.store.get_changeset(ns, c)? {
                for p in cs.parents {
                    stack.push(p);
                }
            }
        }
        let mut best: Option<(u64, ChangesetId)> = None;
        let mut seen = std::collections::BTreeSet::new();
        let mut stack = vec![b];
        while let Some(c) = stack.pop() {
            if !seen.insert(c) {
                continue;
            }
            if aset.contains(&c) {
                let gen = self.graph(ns, c)?.generation;
                if best.map(|(g, _)| gen > g).unwrap_or(true) {
                    best = Some((gen, c));
                }
                continue;
            }
            if let Some(cs) = self.store.get_changeset(ns, c)? {
                for p in cs.parents {
                    stack.push(p);
                }
            }
        }
        Ok(best.map(|(_, id)| id))
    }

    pub fn log(
        &self,
        ns: &str,
        start: ChangesetId,
        limit: usize,
    ) -> scion_store::Result<Vec<Changeset>> {
        self.store.list_first_parent_chain(ns, start, limit)
    }

    pub fn path_history(
        &self,
        ns: &str,
        path: &RepoPath,
        limit: i64,
    ) -> scion_store::Result<Vec<ChangesetId>> {
        self.store.path_history(ns, path.as_str(), limit)
    }

    pub fn blame(&self, ns: &str, cs: ChangesetId, path: &RepoPath) -> scion_store::Result<Blame> {
        let key = blame_key(ns, cs, path);
        if let Some(bytes) = self.store.derived_get(BLAME, VER, &key)? {
            return Ok(decode_blame(&bytes));
        }
        let value = self.compute_blame(ns, cs, path)?;
        self.store
            .derived_put(BLAME, VER, &key, &encode_blame(&value))?;
        Ok(value)
    }

    pub fn diff(
        &self,
        ns: &str,
        from: Option<ChangesetId>,
        to: ChangesetId,
    ) -> scion_store::Result<Vec<DiffFile>> {
        let to_root = self.manifest(ns, to)?;
        let from_root = match from {
            Some(id) => Some(self.manifest(ns, id)?),
            None => None,
        };
        let changes = changes_between(from_root, to_root, |id| self.load_tree(id))?;
        let mut files = Vec::new();
        for (path, change) in changes {
            let (status, patch) = match &change {
                Change::Delete => ("deleted", String::new()),
                Change::Put { blob, .. } => {
                    let new_bytes = self.store.get_blob(*blob).unwrap_or_default();
                    let old_bytes = if let Some(from) = from {
                        self.read_file(ns, from, &path)?.unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    let status = if from.is_some() && !old_bytes.is_empty() {
                        "modified"
                    } else {
                        "added"
                    };
                    let patch = if is_text(&old_bytes) && is_text(&new_bytes) {
                        diffy::create_patch(
                            &String::from_utf8_lossy(&old_bytes),
                            &String::from_utf8_lossy(&new_bytes),
                        )
                        .to_string()
                    } else {
                        format!("binary {} bytes", new_bytes.len())
                    };
                    (status, patch)
                }
            };
            files.push(DiffFile {
                path: path.to_string(),
                status,
                patch,
            });
        }
        Ok(files)
    }

    fn compute_blame(
        &self,
        ns: &str,
        cs: ChangesetId,
        path: &RepoPath,
    ) -> scion_store::Result<Blame> {
        let Some(bytes) = self.read_file(ns, cs, path)? else {
            return Ok(Blame {
                binary: false,
                ranges: Vec::new(),
            });
        };
        if !is_text(&bytes) {
            return Ok(Blame {
                binary: true,
                ranges: Vec::new(),
            });
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let changeset = self
            .store
            .get_changeset(ns, cs)?
            .ok_or(scion_core::Error::NotFound)?;
        let touched = changeset.changes.contains_key(path);
        if !touched {
            if let Some(prev) = self.previous_touch(ns, cs, path)? {
                return self.blame(ns, prev, path);
            }
        }
        let parent = changeset.first_parent();
        let old = match parent {
            Some(p) => self.read_file(ns, p, path)?.unwrap_or_default(),
            None => Vec::new(),
        };
        if parent.is_none() || old.is_empty() && !touched {
            return Ok(all_lines(cs, &text));
        }
        let old_text = String::from_utf8_lossy(&old);
        let parent_blame = match parent {
            Some(p) if !old.is_empty() => self.blame(ns, p, path)?,
            _ => Blame {
                binary: false,
                ranges: Vec::new(),
            },
        };
        let mut lines: Vec<(ChangesetId, u32)> = Vec::new();
        let diff = TextDiff::from_lines(old_text.as_ref(), text.as_str());
        let mut old_line: u32 = 1;
        let mut new_line: u32 = 1;
        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Equal => {
                    let origin = origin_of(&parent_blame, old_line)
                        .unwrap_or((parent.unwrap_or(cs), old_line));
                    lines.push((origin.0, origin.1));
                    old_line += 1;
                    new_line += 1;
                }
                ChangeTag::Delete => {
                    old_line += 1;
                }
                ChangeTag::Insert => {
                    lines.push((cs, new_line));
                    new_line += 1;
                }
            }
        }
        if lines.is_empty() && !text.is_empty() {
            return Ok(all_lines(cs, &text));
        }
        Ok(Blame {
            binary: false,
            ranges: compress_lines(lines),
        })
    }

    fn previous_touch(
        &self,
        ns: &str,
        cs: ChangesetId,
        path: &RepoPath,
    ) -> scion_store::Result<Option<ChangesetId>> {
        let seq = self.store.changeset_seq(ns, cs)?.unwrap_or(i64::MAX);
        let hist = self.store.path_history(ns, path.as_str(), 10_000)?;
        // path_history is newest first, but we don't have seq in the result.
        // Walk first parents until one changes the path.
        let mut cur = self
            .store
            .get_changeset(ns, cs)?
            .and_then(|c| c.first_parent());
        let mut guard = 0;
        while let Some(id) = cur {
            guard += 1;
            if guard > 100_000 {
                break;
            }
            if let Some(prev) = self.store.get_changeset(ns, id)? {
                if prev.changes.contains_key(path) {
                    return Ok(Some(id));
                }
                cur = prev.first_parent();
            } else {
                break;
            }
        }
        let _ = (seq, hist);
        Ok(None)
    }

    fn materialize(&self, cs: &Changeset, parent: Option<TreeId>) -> scion_store::Result<TreeId> {
        let store = &self.store;
        let id = apply_changes(
            parent,
            &cs.changes,
            |tid| store.get_tree(tid).map_err(|_| scion_core::Error::NotFound),
            |tree| {
                store
                    .put_tree(tree)
                    .map_err(|_| scion_core::Error::Other("store tree".into()))
            },
        )?;
        Ok(id)
    }

    fn note_paths(&self, ns: &str, cs: &Changeset) -> scion_store::Result<()> {
        let seq = self.store.changeset_seq(ns, cs.id)?.unwrap_or(0);
        for path in cs.changes.keys() {
            self.store.record_path(ns, path.as_str(), seq, cs.id)?;
        }
        Ok(())
    }

    fn store_manifest(&self, ns: &str, cs: ChangesetId, tree: TreeId) -> scion_store::Result<()> {
        let mut key = Vec::new();
        key.extend_from_slice(ns.as_bytes());
        key.push(0);
        key.extend_from_slice(cs.as_bytes());
        self.store
            .derived_put(MANIFEST, VER, &key, tree.as_bytes())?;
        Ok(())
    }

    fn load_manifest(&self, ns: &str, cs: ChangesetId) -> scion_store::Result<Option<TreeId>> {
        let mut key = Vec::new();
        key.extend_from_slice(ns.as_bytes());
        key.push(0);
        key.extend_from_slice(cs.as_bytes());
        Ok(self.store.derived_get(MANIFEST, VER, &key)?.map(|b| {
            let mut id = [0u8; 32];
            id.copy_from_slice(&b[..32]);
            TreeId::from_bytes(id)
        }))
    }

    fn store_graph(&self, ns: &str, cs: &Changeset) -> scion_store::Result<()> {
        let (generation, skips) = if let Some(parent) = cs.first_parent() {
            let pg = self.load_graph(ns, parent)?.unwrap_or(GraphValue {
                generation: 1,
                skips: Vec::new(),
            });
            let mut skips = vec![parent];
            let mut i = 0;
            while let Some(id) = skips.get(i).copied() {
                if let Some(g) = self.load_graph(ns, id)? {
                    if let Some(next) = g.skips.get(i).copied() {
                        skips.push(next);
                        i += 1;
                        continue;
                    }
                }
                break;
            }
            (pg.generation + 1, skips)
        } else {
            (1, Vec::new())
        };
        let mut val = Vec::new();
        val.extend_from_slice(&generation.to_le_bytes());
        val.extend_from_slice(&(skips.len() as u32).to_le_bytes());
        for s in &skips {
            val.extend_from_slice(s.as_bytes());
        }
        self.store
            .derived_put(GRAPH, VER, &graph_key(ns, cs.id), &val)?;
        Ok(())
    }

    fn load_graph(&self, ns: &str, id: ChangesetId) -> scion_store::Result<Option<GraphValue>> {
        let Some(val) = self.store.derived_get(GRAPH, VER, &graph_key(ns, id))? else {
            return Ok(None);
        };
        if val.len() < 12 {
            return Ok(None);
        }
        let generation = u64::from_le_bytes(val[0..8].try_into().unwrap());
        let n = u32::from_le_bytes(val[8..12].try_into().unwrap()) as usize;
        let mut skips = Vec::new();
        for i in 0..n {
            let start = 12 + i * 32;
            if start + 32 > val.len() {
                break;
            }
            let mut idb = [0u8; 32];
            idb.copy_from_slice(&val[start..start + 32]);
            skips.push(ChangesetId::from_bytes(idb));
        }
        Ok(Some(GraphValue { generation, skips }))
    }

    fn store_changed(&self, ns: &str, cs: &Changeset) -> scion_store::Result<()> {
        let mut filter = [0u8; 64];
        let mut body = Vec::new();
        body.extend_from_slice(&(cs.changes.len() as u32).to_le_bytes());
        for path in cs.changes.keys() {
            bloom_insert(&mut filter, path.as_str());
            for prefix in path.prefixes() {
                bloom_insert(&mut filter, prefix.as_str());
            }
            let b = path.as_str().as_bytes();
            body.extend_from_slice(&(b.len() as u32).to_le_bytes());
            body.extend_from_slice(b);
        }
        body.extend_from_slice(&filter);
        self.store
            .derived_put(CHANGED, VER, &graph_key(ns, cs.id), &body)?;
        // path_log: previous touch per path, encoded as path\\0 + id
        let mut log = Vec::new();
        for path in cs.changes.keys() {
            let prev = self.previous_touch(ns, cs.id, path)?;
            let b = path.as_str().as_bytes();
            log.extend_from_slice(&(b.len() as u32).to_le_bytes());
            log.extend_from_slice(b);
            match prev {
                Some(id) => log.extend_from_slice(id.as_bytes()),
                None => log.extend_from_slice(&[0u8; 32]),
            }
        }
        self.store
            .derived_put(PATH_LOG, VER, &graph_key(ns, cs.id), &log)?;
        Ok(())
    }

    fn load_tree(&self, id: TreeId) -> scion_core::Result<Tree> {
        self.store
            .get_tree(id)
            .map_err(|_| scion_core::Error::NotFound)
    }
}

fn graph_key(ns: &str, id: ChangesetId) -> Vec<u8> {
    let mut key = Vec::new();
    key.extend_from_slice(ns.as_bytes());
    key.push(0);
    key.extend_from_slice(id.as_bytes());
    key
}

fn blame_key(ns: &str, cs: ChangesetId, path: &RepoPath) -> Vec<u8> {
    let mut key = graph_key(ns, cs);
    key.extend_from_slice(path.as_str().as_bytes());
    key
}

fn bloom_insert(filter: &mut [u8; 64], s: &str) {
    let hash = blake3::hash(s.as_bytes());
    let b = hash.as_bytes();
    for i in 0..4 {
        let idx = u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]) as usize % (64 * 8);
        filter[idx / 8] |= 1 << (idx % 8);
    }
}

fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

fn all_lines(cs: ChangesetId, text: &str) -> Blame {
    let count = text.lines().count() as u32;
    if count == 0 {
        return Blame {
            binary: false,
            ranges: Vec::new(),
        };
    }
    Blame {
        binary: false,
        ranges: vec![BlameRange {
            start_line: 1,
            count,
            origin: cs,
            origin_start: 1,
        }],
    }
}

fn origin_of(blame: &Blame, line: u32) -> Option<(ChangesetId, u32)> {
    for range in &blame.ranges {
        if line >= range.start_line && line < range.start_line + range.count {
            let offset = line - range.start_line;
            return Some((range.origin, range.origin_start + offset));
        }
    }
    None
}

fn compress_lines(lines: Vec<(ChangesetId, u32)>) -> Vec<BlameRange> {
    let mut out = Vec::new();
    for (i, (origin, origin_line)) in lines.into_iter().enumerate() {
        let line = (i as u32) + 1;
        if let Some(last) = out.last_mut() {
            let BlameRange {
                start_line,
                count,
                origin: o,
                origin_start,
            } = *last;
            if o == origin && origin_line == origin_start + count && line == start_line + count {
                last.count += 1;
                continue;
            }
        }
        out.push(BlameRange {
            start_line: line,
            count: 1,
            origin,
            origin_start: origin_line,
        });
    }
    out
}

fn encode_blame(b: &Blame) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(u8::from(b.binary));
    v.extend_from_slice(&(b.ranges.len() as u32).to_le_bytes());
    for r in &b.ranges {
        v.extend_from_slice(&r.start_line.to_le_bytes());
        v.extend_from_slice(&r.count.to_le_bytes());
        v.extend_from_slice(r.origin.as_bytes());
        v.extend_from_slice(&r.origin_start.to_le_bytes());
    }
    v
}

fn decode_blame(bytes: &[u8]) -> Blame {
    if bytes.is_empty() {
        return Blame {
            binary: false,
            ranges: Vec::new(),
        };
    }
    let binary = bytes[0] != 0;
    let n = u32::from_le_bytes(bytes[1..5].try_into().unwrap_or([0; 4])) as usize;
    let mut ranges = Vec::new();
    let mut pos = 5;
    for _ in 0..n {
        if pos + 4 + 4 + 32 + 4 > bytes.len() {
            break;
        }
        let start_line = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let count = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes[pos..pos + 32]);
        pos += 32;
        let origin_start = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;
        ranges.push(BlameRange {
            start_line,
            count,
            origin: ChangesetId::from_bytes(id),
            origin_start,
        });
    }
    Blame { binary, ranges }
}

/// True when a changed-path bloom cannot rule out `path`.
pub fn bloom_maybe(filter: &[u8; 64], path: &str) -> bool {
    let hash = blake3::hash(path.as_bytes());
    let b = hash.as_bytes();
    for i in 0..4 {
        let idx = u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]) as usize % (64 * 8);
        if filter[idx / 8] & (1 << (idx % 8)) == 0 {
            return false;
        }
    }
    true
}
