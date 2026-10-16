//! HTTP client, sparse checkout filter, and workspace index.
//!
//! This crate must not depend on the server, the store, or IAM. Authorization
//! happens on the server; a 404 means the repo is missing or unreadable.

mod client;
mod sparse;
mod workspace;

pub use client::Client;
pub use sparse::SparseSpec;
pub use workspace::{diff_index, scion_home, IndexEntry, StatusLine, Workspace};

pub fn repo_spec(spec: &str) -> Option<(&str, &str)> {
    let (owner, name) = spec.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        None
    } else {
        Some((owner, name))
    }
}
