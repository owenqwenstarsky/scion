//! Loose-object Git bridge.
//!
//! Commits written by `git` and not yet packed live under `objects/aa/bb…`.
//! This crate reads and writes that form. Packfiles, alternates, and shallow
//! clones are not implemented; import a repo with `git unpack-objects` first
//! if its history is packed.

use sha1::{Digest, Sha1};
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitFile {
    pub path: String,
    pub executable: bool,
    pub bytes: Vec<u8>,
}

pub fn hash_object(kind: &str, payload: &[u8]) -> (String, Vec<u8>) {
    let mut raw = format!("{kind} {}\0", payload.len()).into_bytes();
    raw.extend_from_slice(payload);
    let digest = Sha1::digest(&raw);
    (hex::encode(digest), raw)
}

pub fn export_snapshot(
    root: &Path,
    files: &[GitFile],
    message: &str,
    author: &str,
) -> Result<String, String> {
    let mut entries = Vec::new();
    for file in files {
        let (blob, raw) = hash_object("blob", &file.bytes);
        write_loose(root, &blob, &raw)?;
        let mode = if file.executable { "100755" } else { "100644" };
        entries.push((mode.to_string(), file.path.clone(), blob, false));
    }
    let tree = write_tree(root, &entries)?;
    let payload = format!("tree {tree}\nauthor {author}\ncommitter {author}\n\n{message}\n");
    let (commit, raw) = hash_object("commit", payload.as_bytes());
    write_loose(root, &commit, &raw)?;
    fs::create_dir_all(root.join("refs/heads")).map_err(|err| err.to_string())?;
    fs::write(root.join("refs/heads/main"), format!("{commit}\n"))
        .map_err(|err| err.to_string())?;
    Ok(commit)
}

fn write_tree(root: &Path, entries: &[(String, String, String, bool)]) -> Result<String, String> {
    let mut sorted = entries.to_vec();
    sorted.sort_by_key(|entry| git_key(&entry.1, entry.3));
    let mut payload = Vec::new();
    for (mode, name, oid, _) in sorted {
        payload.extend_from_slice(mode.as_bytes());
        payload.push(b' ');
        payload.extend_from_slice(name.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&oid_bytes(&oid)?);
    }
    let (tree, raw) = hash_object("tree", &payload);
    write_loose(root, &tree, &raw)?;
    Ok(tree)
}

fn git_key(name: &str, is_dir: bool) -> String {
    if is_dir {
        format!("{name}/")
    } else {
        name.to_string()
    }
}

fn oid_bytes(hex_oid: &str) -> Result<Vec<u8>, String> {
    hex::decode(hex_oid).map_err(|err| err.to_string())
}

fn write_loose(root: &Path, oid: &str, raw: &[u8]) -> Result<(), String> {
    let (prefix, rest) = oid.split_at(2);
    let dir = root.join("objects").join(prefix);
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    fs::write(dir.join(rest), raw).map_err(|err| err.to_string())
}

pub fn import_commit(root: &Path, commit: &str) -> Result<Vec<GitFile>, String> {
    let raw = read_loose(root, commit)?;
    let payload = payload_of(&raw, "commit")?;
    let text = String::from_utf8(payload).map_err(|err| err.to_string())?;
    let tree = text
        .lines()
        .find_map(|line| line.strip_prefix("tree "))
        .ok_or("commit has no tree")?
        .to_string();
    let mut files = Vec::new();
    read_tree(root, &tree, "", &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn read_tree(root: &Path, oid: &str, prefix: &str, out: &mut Vec<GitFile>) -> Result<(), String> {
    let raw = read_loose(root, oid)?;
    let payload = payload_of(&raw, "tree")?;
    let mut i = 0;
    while i < payload.len() {
        let space = payload[i..]
            .iter()
            .position(|b| *b == b' ')
            .ok_or("bad tree")?;
        let mode = std::str::from_utf8(&payload[i..i + space]).map_err(|err| err.to_string())?;
        i += space + 1;
        let nul = payload[i..]
            .iter()
            .position(|b| *b == 0)
            .ok_or("bad tree")?;
        let name = std::str::from_utf8(&payload[i..i + nul])
            .map_err(|err| err.to_string())?
            .to_string();
        i += nul + 1;
        if i + 20 > payload.len() {
            return Err("truncated tree".into());
        }
        let child = hex::encode(&payload[i..i + 20]);
        i += 20;
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if mode == "40000" {
            read_tree(root, &child, &path, out)?;
        } else {
            let blob = read_loose(root, &child)?;
            let bytes = payload_of(&blob, "blob")?;
            out.push(GitFile {
                path,
                executable: mode == "100755",
                bytes,
            });
        }
    }
    Ok(())
}

fn read_loose(root: &Path, oid: &str) -> Result<Vec<u8>, String> {
    let (prefix, rest) = oid.split_at(2);
    fs::read(root.join("objects").join(prefix).join(rest)).map_err(|err| format!("{oid}: {err}"))
}

fn payload_of(raw: &[u8], kind: &str) -> Result<Vec<u8>, String> {
    let nul = raw.iter().position(|b| *b == 0).ok_or("missing header")?;
    let header = std::str::from_utf8(&raw[..nul]).map_err(|err| err.to_string())?;
    if !header.starts_with(kind) {
        return Err(format!("expected {kind}, got {header}"));
    }
    Ok(raw[nul + 1..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_loose_objects() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![
            GitFile {
                path: "README".into(),
                executable: false,
                bytes: b"hello\n".to_vec(),
            },
            GitFile {
                path: "src/main.rs".into(),
                executable: false,
                bytes: b"fn main() {}\n".to_vec(),
            },
        ];
        let commit = export_snapshot(
            dir.path(),
            &files,
            "initial",
            "Ada <ada@example.com> 1700000000 +0000",
        )
        .unwrap();
        let imported = import_commit(dir.path(), &commit).unwrap();
        assert_eq!(imported, files);
    }
}
