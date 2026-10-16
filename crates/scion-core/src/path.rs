use crate::error::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Repository-relative path. Normalized, `/`-separated, no `.` or `..`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoPath(String);

impl RepoPath {
    pub fn new(raw: impl AsRef<str>) -> Result<Self> {
        let normalized = normalize(raw.as_ref())?;
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    pub fn parent(&self) -> Option<RepoPath> {
        self.0.rfind('/').map(|i| RepoPath(self.0[..i].to_string()))
    }

    /// Every strict directory prefix, root-most first (`a`, `a/b` for `a/b/c`).
    pub fn prefixes(&self) -> Vec<RepoPath> {
        let segs: Vec<&str> = self.segments().collect();
        let mut out = Vec::new();
        for i in 1..segs.len() {
            out.push(RepoPath(segs[..i].join("/")));
        }
        out
    }
}

fn normalize(raw: &str) -> Result<String> {
    if raw.is_empty() {
        return Err(Error::Invalid("empty path".into()));
    }
    if raw.contains('\0') {
        return Err(Error::Invalid("path contains NUL".into()));
    }
    if raw.starts_with('/') || raw.contains('\\') {
        return Err(Error::Invalid(
            "path must be relative and use '/' separators".into(),
        ));
    }
    let mut parts = Vec::new();
    for seg in raw.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(Error::Invalid(format!("illegal path segment in `{raw}`")));
        }
        if seg.contains('\0') {
            return Err(Error::Invalid("path contains NUL".into()));
        }
        parts.push(seg);
    }
    Ok(parts.join("/"))
}

impl fmt::Debug for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RepoPath({})", self.0)
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RepoPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for RepoPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RepoPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        RepoPath::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_nested() {
        let p = RepoPath::new("src/lib.rs").unwrap();
        assert_eq!(p.as_str(), "src/lib.rs");
        assert_eq!(p.file_name(), "lib.rs");
        assert_eq!(p.parent().unwrap().as_str(), "src");
    }

    #[test]
    fn rejects_bad() {
        for bad in ["", "/abs", "a/../b", "a//b", "./a", "..", "a\\b", "a/\0/b"] {
            assert!(RepoPath::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn normalize_idempotent() {
        let p = RepoPath::new("a/b/c").unwrap();
        let again = RepoPath::new(p.as_str()).unwrap();
        assert_eq!(p, again);
    }
}
