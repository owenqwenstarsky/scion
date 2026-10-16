use crate::sparse::SparseSpec;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub fn scion_home() -> PathBuf {
    if let Some(home) = std::env::var_os("SCION_HOME") {
        return PathBuf::from(home);
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(home).join(".scion-vcs")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexEntry {
    pub path: String,
    pub blob: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusLine {
    pub path: String,
    pub state: &'static str,
}

/// Compare a checkout index with the files currently on disk (already hashed).
pub fn diff_index(
    index: &[IndexEntry],
    local: &[(String, String)],
    sparse: &SparseSpec,
) -> Vec<StatusLine> {
    let mut lines = Vec::new();
    for (path, blob) in local {
        if !sparse.matches(path) {
            continue;
        }
        match index.iter().find(|entry| entry.path == *path) {
            Some(entry) if &entry.blob == blob => {}
            Some(_) => lines.push(StatusLine {
                path: path.clone(),
                state: "modified",
            }),
            None => lines.push(StatusLine {
                path: path.clone(),
                state: "added",
            }),
        }
    }
    for entry in index {
        if !sparse.matches(&entry.path) {
            continue;
        }
        if !local.iter().any(|(path, _)| path == &entry.path) {
            lines.push(StatusLine {
                path: entry.path.clone(),
                state: "deleted",
            });
        }
    }
    lines.sort_by(|a, b| a.path.cmp(&b.path));
    lines
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub server: String,
    pub owner: String,
    pub name: String,
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub tip: Option<String>,
    pub pending: Option<String>,
}

impl Workspace {
    pub fn config_path(root: &Path) -> PathBuf {
        root.join(".scion").join("repo.json")
    }

    pub fn index_path(root: &Path) -> PathBuf {
        root.join(".scion").join("index.json")
    }

    pub fn sparse_path(root: &Path) -> PathBuf {
        root.join(".scion").join("sparse")
    }

    pub fn load(root: &Path) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(Self::config_path(root)).map_err(|err| err.to_string())?;
        serde_json::from_str(&text).map_err(|err| err.to_string())
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        let path = Self::config_path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|err| err.to_string())?;
        std::fs::write(path, text).map_err(|err| err.to_string())
    }

    pub fn load_sparse(root: &Path) -> Result<SparseSpec, String> {
        match std::fs::read_to_string(Self::sparse_path(root)) {
            Ok(text) => SparseSpec::parse(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(SparseSpec::full()),
            Err(err) => Err(err.to_string()),
        }
    }

    pub fn load_index(root: &Path) -> Result<Vec<IndexEntry>, String> {
        match std::fs::read_to_string(Self::index_path(root)) {
            Ok(text) => serde_json::from_str(&text).map_err(|err| err.to_string()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => Err(err.to_string()),
        }
    }

    pub fn save_index(root: &Path, index: &[IndexEntry]) -> Result<(), String> {
        let path = Self::index_path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        std::fs::write(
            path,
            serde_json::to_vec_pretty(index).map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reports_add_modify_delete() {
        let index = vec![
            IndexEntry {
                path: "a".into(),
                blob: "1".into(),
            },
            IndexEntry {
                path: "b".into(),
                blob: "2".into(),
            },
        ];
        let local = vec![("a".into(), "9".into()), ("c".into(), "3".into())];
        let lines = diff_index(&index, &local, &SparseSpec::full());
        assert_eq!(lines[0].state, "modified");
        assert_eq!(lines[1].state, "deleted");
        assert_eq!(lines[2].state, "added");
    }

    #[test]
    fn sparse_hides_unselected_paths() {
        let index = vec![IndexEntry {
            path: "notes/a".into(),
            blob: "1".into(),
        }];
        let local = vec![
            ("notes/a".into(), "9".into()),
            ("src/lib.rs".into(), "3".into()),
        ];
        let spec = SparseSpec::parse("include src/**\n").unwrap();
        let lines = diff_index(&index, &local, &spec);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].path, "src/lib.rs");
    }
}
