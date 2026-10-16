//! In-memory virtual checkout: a base snapshot plus an overlay of writes and
//! deletes. FUSE (`fuser`) and NFS (`nfsserve`) are not linked. Those mounts
//! need a host driver this environment does not provide; the model is what
//! the server would project.

use scion_core::RepoPath;
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Overlay {
    Write(Vec<u8>),
    Delete,
}

#[derive(Clone, Debug, Default)]
pub struct Vfs {
    base: BTreeMap<String, Vec<u8>>,
    overlay: BTreeMap<String, Overlay>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("not found")]
    NotFound,
    #[error("is a directory")]
    IsDir,
    #[error("{0}")]
    Invalid(String),
}

impl Vfs {
    pub fn from_base(files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Result<Self, Error> {
        let mut base = BTreeMap::new();
        for (path, bytes) in files {
            RepoPath::new(&path).map_err(|err| Error::Invalid(err.to_string()))?;
            base.insert(path, bytes);
        }
        Ok(Self {
            base,
            overlay: BTreeMap::new(),
        })
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>, Error> {
        RepoPath::new(path).map_err(|err| Error::Invalid(err.to_string()))?;
        match self.overlay.get(path) {
            Some(Overlay::Write(bytes)) => Ok(bytes.clone()),
            Some(Overlay::Delete) => Err(Error::NotFound),
            None => self.base.get(path).cloned().ok_or(Error::NotFound),
        }
    }

    pub fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        RepoPath::new(path).map_err(|err| Error::Invalid(err.to_string()))?;
        self.overlay
            .insert(path.to_string(), Overlay::Write(bytes.to_vec()));
        Ok(())
    }

    pub fn unlink(&mut self, path: &str) -> Result<(), Error> {
        self.read(path)?;
        self.overlay.insert(path.to_string(), Overlay::Delete);
        Ok(())
    }

    pub fn readdir(&self, dir: &str) -> Vec<String> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut names = BTreeMap::new();
        let consider = |path: &str, names: &mut BTreeMap<String, ()>| {
            let rest = if prefix.is_empty() {
                path
            } else if let Some(rest) = path.strip_prefix(&prefix) {
                rest
            } else {
                return;
            };
            if rest.is_empty() {
                return;
            }
            let name = rest.split('/').next().unwrap_or(rest);
            names.insert(name.to_string(), ());
        };
        for path in self.base.keys() {
            consider(path, &mut names);
        }
        for (path, overlay) in &self.overlay {
            match overlay {
                Overlay::Delete => {
                    let rest = if prefix.is_empty() {
                        path.as_str()
                    } else if let Some(rest) = path.strip_prefix(&prefix) {
                        rest
                    } else {
                        continue;
                    };
                    if !rest.contains('/') {
                        names.remove(rest);
                    }
                }
                Overlay::Write(_) => consider(path, &mut names),
            }
        }
        names.into_keys().collect()
    }

    /// Overlay expressed as put/delete pairs `(path, Some(bytes) | None)`.
    pub fn changes(&self) -> Vec<(String, Option<Vec<u8>>)> {
        self.overlay
            .iter()
            .map(|(path, overlay)| match overlay {
                Overlay::Write(bytes) => (path.clone(), Some(bytes.clone())),
                Overlay::Delete => (path.clone(), None),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_matches_a_reference_map() {
        let mut vfs = Vfs::from_base([
            ("a.txt".into(), b"a".to_vec()),
            ("dir/b.txt".into(), b"b".to_vec()),
        ])
        .unwrap();
        let mut reference: BTreeMap<String, Vec<u8>> = BTreeMap::from([
            ("a.txt".to_string(), b"a".to_vec()),
            ("dir/b.txt".to_string(), b"b".to_vec()),
        ]);
        vfs.write("a.txt", b"A").unwrap();
        reference.insert("a.txt".to_string(), b"A".to_vec());
        vfs.write("c.txt", b"c").unwrap();
        reference.insert("c.txt".to_string(), b"c".to_vec());
        vfs.unlink("dir/b.txt").unwrap();
        reference.remove("dir/b.txt");
        for (path, bytes) in &reference {
            assert_eq!(vfs.read(path).unwrap(), *bytes);
        }
        assert!(vfs.read("dir/b.txt").is_err());
        assert_eq!(vfs.readdir(""), vec!["a.txt", "c.txt", "dir"]);
    }
}
