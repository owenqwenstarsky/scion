use globset::{Glob, GlobMatcher};

/// Client-side sparse checkout. The server still authorizes every path; this
/// filter only decides which files the workspace materializes.
#[derive(Clone, Debug)]
pub struct SparseSpec {
    includes: Vec<GlobMatcher>,
    excludes: Vec<GlobMatcher>,
}

impl SparseSpec {
    pub fn full() -> Self {
        Self {
            includes: Vec::new(),
            excludes: Vec::new(),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut includes = Vec::new();
        let mut excludes = Vec::new();
        for (lineno, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (exclude, pattern) = if let Some(rest) = line.strip_prefix('-') {
                (true, rest.trim())
            } else if let Some(rest) = line.strip_prefix('+') {
                (false, rest.trim())
            } else if let Some(rest) = line.strip_prefix("exclude ") {
                (true, rest.trim())
            } else if let Some(rest) = line.strip_prefix("include ") {
                (false, rest.trim())
            } else {
                (false, line)
            };
            let matcher = Glob::new(pattern)
                .map_err(|err| format!("line {}: {err}", lineno + 1))?
                .compile_matcher();
            if exclude {
                excludes.push(matcher);
            } else {
                includes.push(matcher);
            }
        }
        Ok(Self { includes, excludes })
    }

    pub fn matches(&self, path: &str) -> bool {
        if self.excludes.iter().any(|glob| glob.is_match(path)) {
            return false;
        }
        self.includes.is_empty() || self.includes.iter().any(|glob| glob.is_match(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_checkout_matches_everything() {
        let spec = SparseSpec::full();
        assert!(spec.matches("src/main.rs"));
    }

    #[test]
    fn includes_and_excludes() {
        let spec = SparseSpec::parse("include src/**\nexclude src/**/*.log\n").unwrap();
        assert!(spec.matches("src/lib.rs"));
        assert!(!spec.matches("src/debug.log"));
        assert!(!spec.matches("README"));
    }
}
