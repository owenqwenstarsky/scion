//! Content-addressed blobs: small objects in SQLite, larger ones as zstd files,
//! and files above the chunk threshold as FastCDC chunks.

use crate::error::{Error, Result};
use crate::Store;
use fastcdc::v2020::FastCDC;
use rusqlite::OptionalExtension;
use scion_core::{BlobId, ChunkId};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SMALL_MAX: usize = 32 * 1024;
pub const CHUNK_THRESHOLD: usize = 1024 * 1024;
const CDC_MIN: usize = 16 * 1024;
const CDC_AVG: usize = 64 * 1024;
const CDC_MAX: usize = 256 * 1024;
const DECOMPRESS_MAX: usize = 64 * 1024 * 1024;

const LAYOUT_SMALL: i64 = 0;
const LAYOUT_FILE: i64 = 1;
const LAYOUT_CHUNKED: i64 = 2;
const FLAG_ZSTD: i64 = 1;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn looks_compressed(raw: &[u8]) -> bool {
    raw.starts_with(b"\x89PNG")
        || raw.starts_with(&[0xFF, 0xD8, 0xFF])
        || raw.starts_with(b"GIF8")
        || raw.starts_with(b"PK\x03\x04")
        || raw.starts_with(&[0x1F, 0x8B])
        || raw.starts_with(&[0x28, 0xB5, 0x2F, 0xFD])
        || raw.starts_with(b"%PDF")
}

fn compress(raw: &[u8]) -> Result<(Vec<u8>, bool)> {
    if looks_compressed(raw) {
        return Ok((raw.to_vec(), false));
    }
    let compressed = zstd::stream::encode_all(std::io::Cursor::new(raw), 3)?;
    if compressed.len() + 16 >= raw.len() {
        Ok((raw.to_vec(), false))
    } else {
        Ok((compressed, true))
    }
}

fn decompress(data: &[u8], compressed: bool) -> Result<Vec<u8>> {
    if !compressed {
        return Ok(data.to_vec());
    }
    let mut dec = zstd::stream::Decoder::new(std::io::Cursor::new(data))?;
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = dec.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if out.len() + n > DECOMPRESS_MAX {
            return Err(Error::DecompressionLimit(DECOMPRESS_MAX));
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}

fn rel_for(kind: &str, id: &[u8; 32]) -> PathBuf {
    let h = hex::encode(id);
    PathBuf::from(kind)
        .join(&h[..2])
        .join(&h[2..4])
        .join(&h[4..])
}

fn atomic_write(root: &Path, rel: &Path, bytes: &[u8]) -> Result<()> {
    let dest = root.join(rel);
    if dest.exists() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_dir = root.join("tmp");
    fs::create_dir_all(&tmp_dir)?;
    let tmp = tmp_dir.join(format!("w-{}-{}", std::process::id(), now().unsigned_abs()));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &dest)?;
    Ok(())
}

fn read_file(root: &Path, rel: &Path) -> Result<Vec<u8>> {
    Ok(fs::read(root.join(rel))?)
}

impl Store {
    pub fn put_blob(&self, id: BlobId, raw: &[u8]) -> Result<()> {
        if BlobId::of(raw) != id {
            return Err(Error::HashMismatch);
        }
        let mut guard = self.lock();
        if blob_exists(&guard.meta, id)? {
            return Ok(());
        }
        if raw.len() <= SMALL_MAX {
            guard.small.execute(
                "INSERT OR IGNORE INTO small_blobs(id, data) VALUES (?1, ?2)",
                rusqlite::params![id.as_bytes().as_slice(), raw],
            )?;
            insert_index(
                &guard.meta,
                id,
                raw.len() as i64,
                raw.len() as i64,
                LAYOUT_SMALL,
                0,
            )?;
            return Ok(());
        }
        if raw.len() <= CHUNK_THRESHOLD {
            let (stored, zstd) = compress(raw)?;
            let flags = if zstd { FLAG_ZSTD } else { 0 };
            atomic_write(self.root(), &rel_for("blobs", id.as_bytes()), &stored)?;
            insert_index(
                &guard.meta,
                id,
                raw.len() as i64,
                stored.len() as i64,
                LAYOUT_FILE,
                flags,
            )?;
            return Ok(());
        }
        let chunker = FastCDC::new(raw, CDC_MIN, CDC_AVG, CDC_MAX);
        let mut rows: Vec<(i64, [u8; 32], i64, i64)> = Vec::new();
        let mut stored_total = 0i64;
        for (idx, entry) in chunker.enumerate() {
            let slice = &raw[entry.offset..entry.offset + entry.length];
            let cid = ChunkId::of(slice);
            let (stored, zstd) = compress(slice)?;
            let flags = if zstd { FLAG_ZSTD } else { 0 };
            atomic_write(self.root(), &rel_for("chunks", cid.as_bytes()), &stored)?;
            // Chunks are addressed separately; record compression in a sidecar row of blob_index
            // keyed by the chunk id so reads know whether to decompress.
            if !blob_exists(&guard.meta, BlobId::from_bytes(*cid.as_bytes()))? {
                insert_index(
                    &guard.meta,
                    BlobId::from_bytes(*cid.as_bytes()),
                    slice.len() as i64,
                    stored.len() as i64,
                    LAYOUT_FILE,
                    flags,
                )?;
            }
            stored_total += stored.len() as i64;
            rows.push((
                idx as i64,
                *cid.as_bytes(),
                entry.offset as i64,
                entry.length as i64,
            ));
        }
        let tx = guard.meta.transaction()?;
        for (idx, chunk, off, len) in &rows {
            tx.execute(
                "INSERT OR IGNORE INTO blob_manifest(id, idx, chunk, off, len) VALUES (?1,?2,?3,?4,?5)",
                rusqlite::params![id.as_bytes().as_slice(), idx, chunk.as_slice(), off, len],
            )?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO blob_index(id, size, stored_size, layout, flags, created_at)
             VALUES (?1,?2,?3,?4,0,?5)",
            rusqlite::params![
                id.as_bytes().as_slice(),
                raw.len() as i64,
                stored_total,
                LAYOUT_CHUNKED,
                now()
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn has_blob(&self, id: BlobId) -> Result<bool> {
        let guard = self.lock();
        blob_exists(&guard.meta, id)
    }

    pub fn has_blobs(&self, ids: &[BlobId]) -> Result<Vec<bool>> {
        let guard = self.lock();
        ids.iter().map(|id| blob_exists(&guard.meta, *id)).collect()
    }

    pub fn get_blob(&self, id: BlobId) -> Result<Vec<u8>> {
        self.get_blob_range(id, None)
    }

    pub fn get_blob_range(
        &self,
        id: BlobId,
        range: Option<std::ops::Range<u64>>,
    ) -> Result<Vec<u8>> {
        let guard = self.lock();
        let row: Option<(i64, i64, i64)> = guard
            .meta
            .query_row(
                "SELECT size, layout, flags FROM blob_index WHERE id=?1",
                [id.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((size, layout, flags)) = row else {
            return Err(Error::Core(scion_core::Error::NotFound));
        };
        let full = match layout {
            LAYOUT_SMALL => {
                let data: Vec<u8> = guard.small.query_row(
                    "SELECT data FROM small_blobs WHERE id=?1",
                    [id.as_bytes().as_slice()],
                    |r| r.get(0),
                )?;
                data
            }
            LAYOUT_FILE => {
                let bytes = read_file(self.root(), &rel_for("blobs", id.as_bytes()))?;
                decompress(&bytes, flags & FLAG_ZSTD != 0)?
            }
            LAYOUT_CHUNKED => {
                let mut stmt = guard.meta.prepare(
                    "SELECT chunk, off, len FROM blob_manifest WHERE id=?1 ORDER BY idx",
                )?;
                let rows = stmt.query_map([id.as_bytes().as_slice()], |r| {
                    let chunk: Vec<u8> = r.get(0)?;
                    let off: i64 = r.get(1)?;
                    let len: i64 = r.get(2)?;
                    Ok((chunk, off, len))
                })?;
                let mut assembled = vec![0u8; size as usize];
                for row in rows {
                    let (chunk, off, len) = row?;
                    let mut cid = [0u8; 32];
                    cid.copy_from_slice(&chunk);
                    let (cflags,): (i64,) = guard.meta.query_row(
                        "SELECT flags FROM blob_index WHERE id=?1",
                        [cid.as_slice()],
                        |r| Ok((r.get(0)?,)),
                    )?;
                    let stored = read_file(self.root(), &rel_for("chunks", &cid))?;
                    let plain = decompress(&stored, cflags & FLAG_ZSTD != 0)?;
                    if plain.len() != len as usize {
                        return Err(Error::Msg("chunk length mismatch".into()));
                    }
                    let start = off as usize;
                    assembled[start..start + plain.len()].copy_from_slice(&plain);
                }
                assembled
            }
            other => return Err(Error::Msg(format!("unknown blob layout {other}"))),
        };
        if let Some(range) = range {
            let start = range.start.min(full.len() as u64) as usize;
            let end = range.end.min(full.len() as u64) as usize;
            Ok(full[start..end].to_vec())
        } else {
            Ok(full)
        }
    }

    pub fn blob_count(&self) -> Result<i64> {
        let guard = self.lock();
        Ok(guard
            .meta
            .query_row("SELECT COUNT(*) FROM blob_index", [], |r| r.get(0))?)
    }
}

fn blob_exists(meta: &rusqlite::Connection, id: BlobId) -> Result<bool> {
    let n: i64 = meta.query_row(
        "SELECT COUNT(*) FROM blob_index WHERE id=?1",
        [id.as_bytes().as_slice()],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn insert_index(
    meta: &rusqlite::Connection,
    id: BlobId,
    size: i64,
    stored: i64,
    layout: i64,
    flags: i64,
) -> Result<()> {
    meta.execute(
        "INSERT OR IGNORE INTO blob_index(id, size, stored_size, layout, flags, created_at)
         VALUES (?1,?2,?3,?4,?5,?6)",
        rusqlite::params![id.as_bytes().as_slice(), size, stored, layout, flags, now()],
    )?;
    Ok(())
}
