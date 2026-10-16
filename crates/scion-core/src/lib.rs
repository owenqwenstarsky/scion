//! Canonical types for the Scion data model.
//!
//! This crate performs no I/O. Object identity is a domain-separated BLAKE3
//! hash of a hand-written canonical encoding, never of serde output.

mod encoding;
mod error;
mod id;
mod manifest;
mod model;
mod path;

pub use error::{Error, Result};
pub use id::{hash_token, BlobId, ChangesetId, ChunkId, Hash32, TreeId};
pub use manifest::{
    apply_changes, changes_between, diff_trees, empty_tree, lookup_entry, TreeDiff,
};
pub use model::{Change, Changeset, FileKind, FileState, Identity, Tree, TreeEntry};
pub use path::RepoPath;

/// Domain strings. Bumping one of these is a permanent format change.
pub mod domains {
    pub const BLOB: &str = BlobId::DOMAIN;
    pub const CHUNK: &str = ChunkId::DOMAIN;
    pub const TREE: &str = TreeId::DOMAIN;
    pub const CHANGESET: &str = ChangesetId::DOMAIN;
    pub const SESSION: &str = "scion session v1";
    pub const API_TOKEN: &str = "scion api token v1";
    pub const CSRF: &str = "scion csrf v1";

    use super::{BlobId, ChangesetId, ChunkId, TreeId};
}
