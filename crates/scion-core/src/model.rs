use crate::encoding::{Reader, Writer};
use crate::error::{Error, Result};
use crate::id::{BlobId, ChangesetId, TreeId};
use crate::path::RepoPath;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Regular,
    Executable,
    Symlink,
}

impl FileKind {
    fn tag(self) -> u8 {
        match self {
            FileKind::Regular => 1,
            FileKind::Executable => 2,
            FileKind::Symlink => 3,
        }
    }

    fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            1 => Ok(FileKind::Regular),
            2 => Ok(FileKind::Executable),
            3 => Ok(FileKind::Symlink),
            _ => Err(Error::Encoding(format!("bad file kind {tag}"))),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Change {
    Put {
        kind: FileKind,
        blob: BlobId,
        size: u64,
    },
    Delete,
}

impl Change {
    fn encode(&self, w: &mut Writer) {
        match self {
            Change::Delete => w.u8(2),
            Change::Put { kind, blob, size } => {
                w.u8(1);
                w.u8(kind.tag());
                w.raw(blob.as_bytes());
                w.u64(*size);
            }
        }
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        match r.u8()? {
            2 => Ok(Change::Delete),
            1 => {
                let kind = FileKind::from_tag(r.u8()?)?;
                let mut id = [0u8; 32];
                id.copy_from_slice(r.raw(32)?);
                let size = r.u64()?;
                Ok(Change::Put {
                    kind,
                    blob: BlobId::from_bytes(id),
                    size,
                })
            }
            t => Err(Error::Encoding(format!("bad change tag {t}"))),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub email: String,
    pub when: i64,
    pub tz_min: i16,
}

impl Identity {
    pub fn new(name: impl Into<String>, email: impl Into<String>, when: i64) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
            when,
            tz_min: 0,
        }
    }

    fn encode(&self, w: &mut Writer) {
        w.str(&self.name);
        w.str(&self.email);
        w.i64(self.when);
        w.i16(self.tz_min);
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            name: r.str()?,
            email: r.str()?,
            when: r.i64()?,
            tz_min: r.i16()?,
        })
    }
}

/// A changeset is a write-ahead-log entry: file changes against the first parent.
///
/// The id is the BLAKE3 hash of the canonical encoding and is not stored inside it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Changeset {
    pub id: ChangesetId,
    pub parents: Vec<ChangesetId>,
    pub changes: BTreeMap<RepoPath, Change>,
    pub author: Identity,
    pub committer: Identity,
    pub message: String,
    pub extra: BTreeMap<String, Vec<u8>>,
}

impl Changeset {
    pub fn create(
        parents: Vec<ChangesetId>,
        changes: BTreeMap<RepoPath, Change>,
        author: Identity,
        committer: Identity,
        message: impl Into<String>,
        extra: BTreeMap<String, Vec<u8>>,
    ) -> Self {
        let message = message.into();
        let partial = Changeset {
            id: ChangesetId::from_bytes([0; 32]),
            parents,
            changes,
            author,
            committer,
            message,
            extra,
        };
        let id = ChangesetId::of(&partial.encode());
        Changeset { id, ..partial }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.raw(b"SCNC");
        w.u8(1);
        w.u32(self.parents.len() as u32);
        for p in &self.parents {
            w.raw(p.as_bytes());
        }
        w.u32(self.changes.len() as u32);
        for (path, change) in &self.changes {
            w.str(path.as_str());
            change.encode(&mut w);
        }
        self.author.encode(&mut w);
        self.committer.encode(&mut w);
        w.str(&self.message);
        w.u32(self.extra.len() as u32);
        for (k, v) in &self.extra {
            w.str(k);
            w.bytes(v);
        }
        w.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let magic = r.raw(4)?;
        if magic != b"SCNC" {
            return Err(Error::Encoding("bad changeset magic".into()));
        }
        let ver = r.u8()?;
        if ver != 1 {
            return Err(Error::Encoding(format!(
                "unsupported changeset version {ver}"
            )));
        }
        let nparents = r.u32()? as usize;
        let mut parents = Vec::with_capacity(nparents);
        for _ in 0..nparents {
            let mut id = [0u8; 32];
            id.copy_from_slice(r.raw(32)?);
            parents.push(ChangesetId::from_bytes(id));
        }
        let nchanges = r.u32()? as usize;
        let mut changes = BTreeMap::new();
        let mut prev = String::new();
        for i in 0..nchanges {
            let path = RepoPath::new(r.str()?)?;
            if i > 0 && path.as_str() <= prev.as_str() {
                return Err(Error::Encoding("changes not strictly sorted".into()));
            }
            prev = path.as_str().to_string();
            let change = Change::decode(&mut r)?;
            changes.insert(path, change);
        }
        let author = Identity::decode(&mut r)?;
        let committer = Identity::decode(&mut r)?;
        let message = r.str()?;
        let nextra = r.u32()? as usize;
        let mut extra = BTreeMap::new();
        let mut prev_k = String::new();
        for i in 0..nextra {
            let k = r.str()?;
            if i > 0 && k.as_str() <= prev_k.as_str() {
                return Err(Error::Encoding("extra not strictly sorted".into()));
            }
            prev_k = k.clone();
            extra.insert(k, r.bytes()?.to_vec());
        }
        r.finish()?;
        let cs = Changeset {
            id: ChangesetId::from_bytes([0; 32]),
            parents,
            changes,
            author,
            committer,
            message,
            extra,
        };
        let id = ChangesetId::of(&cs.encode());
        Ok(Changeset { id, ..cs })
    }

    pub fn first_parent(&self) -> Option<ChangesetId> {
        self.parents.first().copied()
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TreeEntry {
    File {
        kind: FileKind,
        blob: BlobId,
        size: u64,
    },
    Dir {
        tree: TreeId,
    },
}

impl TreeEntry {
    pub fn is_dir(&self) -> bool {
        matches!(self, TreeEntry::Dir { .. })
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Tree {
    pub entries: BTreeMap<String, TreeEntry>,
}

impl Tree {
    pub fn empty() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    pub fn id(&self) -> TreeId {
        TreeId::of(&self.encode())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.raw(b"SCNT");
        w.u8(1);
        w.u32(self.entries.len() as u32);
        for (name, entry) in &self.entries {
            w.str(name);
            match entry {
                TreeEntry::File { kind, blob, size } => {
                    w.u8(1);
                    w.u8(kind.tag());
                    w.raw(blob.as_bytes());
                    w.u64(*size);
                }
                TreeEntry::Dir { tree } => {
                    w.u8(2);
                    w.raw(tree.as_bytes());
                }
            }
        }
        w.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let magic = r.raw(4)?;
        if magic != b"SCNT" {
            return Err(Error::Encoding("bad tree magic".into()));
        }
        let ver = r.u8()?;
        if ver != 1 {
            return Err(Error::Encoding(format!("unsupported tree version {ver}")));
        }
        let n = r.u32()? as usize;
        let mut entries = BTreeMap::new();
        let mut prev = String::new();
        for i in 0..n {
            let name = r.str()?;
            if name.is_empty() || name.contains('/') || name == "." || name == ".." {
                return Err(Error::Encoding(format!("bad entry name `{name}`")));
            }
            if i > 0 && name.as_str() <= prev.as_str() {
                return Err(Error::Encoding("tree entries not strictly sorted".into()));
            }
            prev = name.clone();
            let entry = match r.u8()? {
                1 => {
                    let kind = FileKind::from_tag(r.u8()?)?;
                    let mut id = [0u8; 32];
                    id.copy_from_slice(r.raw(32)?);
                    TreeEntry::File {
                        kind,
                        blob: BlobId::from_bytes(id),
                        size: r.u64()?,
                    }
                }
                2 => {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(r.raw(32)?);
                    TreeEntry::Dir {
                        tree: TreeId::from_bytes(id),
                    }
                }
                t => return Err(Error::Encoding(format!("bad tree entry tag {t}"))),
            };
            entries.insert(name, entry);
        }
        r.finish()?;
        Ok(Tree { entries })
    }
}

/// File content state at a single path, used by rebase.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FileState {
    Absent,
    File {
        kind: FileKind,
        blob: BlobId,
        size: u64,
    },
}

impl FileState {
    pub fn from_change(change: &Change) -> Self {
        match change {
            Change::Delete => FileState::Absent,
            Change::Put { kind, blob, size } => FileState::File {
                kind: *kind,
                blob: *blob,
                size: *size,
            },
        }
    }

    pub fn from_entry(entry: Option<&TreeEntry>) -> Result<Self> {
        match entry {
            None => Ok(FileState::Absent),
            Some(TreeEntry::File { kind, blob, size }) => Ok(FileState::File {
                kind: *kind,
                blob: *blob,
                size: *size,
            }),
            Some(TreeEntry::Dir { .. }) => Err(Error::Invalid(
                "expected a file but found a directory".into(),
            )),
        }
    }

    pub fn as_change_from_absent(&self) -> Option<Change> {
        match self {
            FileState::Absent => None,
            FileState::File { kind, blob, size } => Some(Change::Put {
                kind: *kind,
                blob: *blob,
                size: *size,
            }),
        }
    }
}
