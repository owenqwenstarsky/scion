//! Substring search over paths and file text the caller already has.
//! A repository-wide trigram index is not built.

pub fn search_paths<'a>(paths: impl IntoIterator<Item = &'a str>, query: &str) -> Vec<&'a str> {
    let _ = scion_core::domains::BLOB;
    let needle = query.to_ascii_lowercase();
    let mut hits: Vec<&str> = paths
        .into_iter()
        .filter(|path| path.to_ascii_lowercase().contains(&needle))
        .collect();
    hits.sort_unstable();
    hits
}

pub fn search_text(files: &[(String, String)], query: &str) -> Vec<(String, u32)> {
    let needle = query.to_ascii_lowercase();
    let mut hits = Vec::new();
    for (path, text) in files {
        for (idx, line) in text.lines().enumerate() {
            if line.to_ascii_lowercase().contains(&needle) {
                hits.push((path.clone(), idx as u32 + 1));
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_paths_and_lines() {
        let paths = search_paths(["src/lib.rs", "README"], "lib");
        assert_eq!(paths, vec!["src/lib.rs"]);
        let hits = search_text(&[("a.txt".into(), "alpha\nBeta\n".into())], "beta");
        assert_eq!(hits, vec![("a.txt".into(), 2)]);
    }
}
