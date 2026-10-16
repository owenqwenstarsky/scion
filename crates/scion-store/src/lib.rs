//! Local metadata (SQLite, WAL) and content-addressed blobs.
//!
//! A single mutex serializes writers. Readers share that lock too: this is the
//! local stand-in for a strongly consistent database, not a connection pool.
//! The async server is expected to call in on `spawn_blocking`.

mod blob;
mod error;
mod schema;

pub use error::{Error, Result};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use schema::{SCHEMA, SMALL_SCHEMA};
use scion_core::{Changeset, ChangesetId, Tree, TreeId};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub use blob::{CHUNK_THRESHOLD, SMALL_MAX};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

struct Conns {
    meta: Connection,
    small: Connection,
}

pub struct Store {
    root: PathBuf,
    conns: Mutex<Conns>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefState {
    pub target: ChangesetId,
    pub version: u64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Landing {
    pub ref_name: String,
    pub changeset: ChangesetId,
    pub landed_as: ChangesetId,
    pub version: u64,
    pub ts: i64,
    pub actor: String,
}

#[derive(Clone, Debug)]
pub struct Cas {
    pub ns: String,
    pub name: String,
    pub expected: Option<ChangesetId>,
    pub new_target: ChangesetId,
    /// Changeset the client originally asked to land (before rebase).
    pub source: ChangesetId,
    pub actor: String,
}

#[derive(Clone, Debug, Default)]
pub struct DoctorReport {
    pub changesets: i64,
    pub refs: i64,
    pub blobs: i64,
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LandEvent {
    pub ref_name: String,
    pub outcome: String,
    pub attempts: i64,
    pub detail: String,
    pub ts: i64,
}

#[derive(Clone, Debug, Default)]
pub struct GcReport {
    pub blobs_deleted: u64,
    pub changesets_deleted: u64,
}

impl Store {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let root = dir.as_ref().to_path_buf();
        fs::create_dir_all(root.join("tmp"))?;
        fs::create_dir_all(root.join("blobs"))?;
        fs::create_dir_all(root.join("chunks"))?;
        fs::create_dir_all(root.join("backups"))?;
        let meta = open_sqlite(&root.join("meta.db"))?;
        meta.execute_batch(SCHEMA)?;
        meta.execute(
            "INSERT OR IGNORE INTO schema_meta(key, value) VALUES ('version', '1')",
            [],
        )?;
        let small = open_sqlite(&root.join("small-blobs.db"))?;
        small.execute_batch(SMALL_SCHEMA)?;
        Ok(Self {
            root,
            conns: Mutex::new(Conns { meta, small }),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Conns> {
        self.conns.lock().expect("store lock poisoned")
    }

    /// Run a closure against the metadata connection (accounts, audit, grants).
    pub fn with_meta<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        let guard = self.lock();
        f(&guard.meta)
    }

    pub fn with_meta_mut<T>(&self, f: impl FnOnce(&mut Connection) -> T) -> T {
        let mut guard = self.lock();
        f(&mut guard.meta)
    }

    pub fn put_tree(&self, tree: &Tree) -> Result<TreeId> {
        let id = tree.id();
        let body = tree.encode();
        let guard = self.lock();
        guard.meta.execute(
            "INSERT OR IGNORE INTO trees(id, body) VALUES (?1, ?2)",
            rusqlite::params![id.as_bytes().as_slice(), body],
        )?;
        Ok(id)
    }

    pub fn get_tree(&self, id: TreeId) -> Result<Tree> {
        let guard = self.lock();
        let body: Option<Vec<u8>> = guard
            .meta
            .query_row(
                "SELECT body FROM trees WHERE id=?1",
                [id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(body) = body else {
            return Err(scion_core::Error::NotFound.into());
        };
        Ok(Tree::decode(&body)?)
    }

    pub fn put_changeset(&self, ns: &str, cs: &Changeset) -> Result<bool> {
        let encoded = cs.encode();
        let id = ChangesetId::of(&encoded);
        if id != cs.id {
            return Err(Error::HashMismatch);
        }
        let mut parents = Vec::with_capacity(cs.parents.len() * 32);
        for p in &cs.parents {
            parents.extend_from_slice(p.as_bytes());
        }
        let body = zstd::stream::encode_all(std::io::Cursor::new(encoded), 3)?;
        let guard = self.lock();
        let existing: Option<i64> = guard
            .meta
            .query_row(
                "SELECT seq FROM changesets WHERE ns=?1 AND id=?2",
                rusqlite::params![ns, id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Ok(false);
        }
        let seq: i64 = guard.meta.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM changesets",
            [],
            |r| r.get(0),
        )?;
        guard.meta.execute(
            "INSERT INTO changesets(ns, id, seq, parents, body, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![ns, id.as_bytes().as_slice(), seq, parents, body, now()],
        )?;
        Ok(true)
    }

    pub fn changeset_seq(&self, ns: &str, id: ChangesetId) -> Result<Option<i64>> {
        let guard = self.lock();
        Ok(guard
            .meta
            .query_row(
                "SELECT seq FROM changesets WHERE ns=?1 AND id=?2",
                rusqlite::params![ns, id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn get_changeset(&self, ns: &str, id: ChangesetId) -> Result<Option<Changeset>> {
        let guard = self.lock();
        let body: Option<Vec<u8>> = guard
            .meta
            .query_row(
                "SELECT body FROM changesets WHERE ns=?1 AND id=?2",
                rusqlite::params![ns, id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(body) = body else {
            return Ok(None);
        };
        let plain = zstd::stream::decode_all(std::io::Cursor::new(body))?;
        Ok(Some(Changeset::decode(&plain)?))
    }

    pub fn list_first_parent_chain(
        &self,
        ns: &str,
        start: ChangesetId,
        limit: usize,
    ) -> Result<Vec<Changeset>> {
        let mut out = Vec::new();
        let mut cursor = Some(start);
        while let Some(id) = cursor {
            if out.len() >= limit {
                break;
            }
            let Some(cs) = self.get_changeset(ns, id)? else {
                break;
            };
            cursor = cs.first_parent();
            out.push(cs);
        }
        Ok(out)
    }

    pub fn get_ref(&self, ns: &str, name: &str) -> Result<Option<RefState>> {
        let guard = self.lock();
        let row: Option<(Vec<u8>, i64, i64)> = guard
            .meta
            .query_row(
                "SELECT target, version, updated_at FROM refs WHERE ns=?1 AND name=?2",
                rusqlite::params![ns, name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(row.map(|(t, v, ts)| RefState {
            target: id_from_vec(&t),
            version: v as u64,
            updated_at: ts,
        }))
    }

    pub fn list_refs(&self, ns: &str) -> Result<Vec<(String, RefState)>> {
        let guard = self.lock();
        let mut stmt = guard.meta.prepare(
            "SELECT name, target, version, updated_at FROM refs WHERE ns=?1 ORDER BY name",
        )?;
        let rows = stmt.query_map([ns], |r| {
            let name: String = r.get(0)?;
            let target: Vec<u8> = r.get(1)?;
            let version: i64 = r.get(2)?;
            let updated: i64 = r.get(3)?;
            Ok((name, target, version, updated))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (name, target, version, updated) = row?;
            out.push((
                name,
                RefState {
                    target: id_from_vec(&target),
                    version: version as u64,
                    updated_at: updated,
                },
            ));
        }
        Ok(out)
    }

    /// Compare-and-swap one ref. On success the ref log and landing row commit
    /// in the same immediate transaction.
    pub fn cas_ref(&self, cas: &Cas) -> Result<bool> {
        let mut guard = self.lock();
        let tx = guard
            .meta
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<Vec<u8>> = tx
            .query_row(
                "SELECT target FROM refs WHERE ns=?1 AND name=?2",
                rusqlite::params![cas.ns, cas.name],
                |r| r.get(0),
            )
            .optional()?;
        let ok = match (&current, &cas.expected) {
            (None, None) => true,
            (Some(have), Some(exp)) => have.as_slice() == exp.as_bytes(),
            _ => false,
        };
        if !ok {
            return Ok(false);
        }
        let ts = now();
        let version: i64 = if current.is_some() {
            let v: i64 = tx.query_row(
                "SELECT version FROM refs WHERE ns=?1 AND name=?2",
                rusqlite::params![cas.ns, cas.name],
                |r| r.get(0),
            )?;
            let n = tx.execute(
                "UPDATE refs SET target=?1, version=?2, updated_at=?3
                 WHERE ns=?4 AND name=?5 AND target=?6",
                rusqlite::params![
                    cas.new_target.as_bytes().as_slice(),
                    v + 1,
                    ts,
                    cas.ns,
                    cas.name,
                    current.as_deref()
                ],
            )?;
            if n != 1 {
                return Ok(false);
            }
            v + 1
        } else {
            tx.execute(
                "INSERT INTO refs(ns, name, target, version, updated_at) VALUES (?1,?2,?3,1,?4)",
                rusqlite::params![cas.ns, cas.name, cas.new_target.as_bytes().as_slice(), ts],
            )?;
            1
        };
        tx.execute(
            "INSERT INTO ref_log(ns, name, old, new, actor, ts) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![
                cas.ns,
                cas.name,
                current,
                cas.new_target.as_bytes().as_slice(),
                cas.actor,
                ts
            ],
        )?;
        tx.execute(
            "INSERT INTO landings(ns, ref, changeset, landed_as, ref_seq, ts, actor)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                cas.ns,
                cas.name,
                cas.source.as_bytes().as_slice(),
                cas.new_target.as_bytes().as_slice(),
                version,
                ts,
                cas.actor
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn when_landed(&self, ns: &str, id: ChangesetId) -> Result<Option<Landing>> {
        let guard = self.lock();
        type LandingRow = (String, Vec<u8>, Vec<u8>, i64, i64, String);
        let row: Option<LandingRow> = guard
            .meta
            .query_row(
                "SELECT ref, changeset, landed_as, ref_seq, ts, actor FROM landings
                 WHERE ns=?1 AND (changeset=?2 OR landed_as=?2)
                 ORDER BY ts LIMIT 1",
                rusqlite::params![ns, id.as_bytes().as_slice()],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.map(|(name, cs, landed, ver, ts, actor)| Landing {
            ref_name: name,
            changeset: id_from_vec(&cs),
            landed_as: id_from_vec(&landed),
            version: ver as u64,
            ts,
            actor,
        }))
    }

    pub fn list_landings(&self, ns: &str, limit: i64) -> Result<Vec<Landing>> {
        let guard = self.lock();
        let mut stmt = guard.meta.prepare(
            "SELECT ref, changeset, landed_as, ref_seq, ts, actor FROM landings
             WHERE ns=?1 ORDER BY ts DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![ns, limit], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (name, cs, landed, ver, ts, actor) = row?;
            out.push(Landing {
                ref_name: name,
                changeset: id_from_vec(&cs),
                landed_as: id_from_vec(&landed),
                version: ver as u64,
                ts,
                actor,
            });
        }
        Ok(out)
    }

    pub fn derived_get(&self, kind: &str, ver: u32, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let guard = self.lock();
        Ok(guard
            .meta
            .query_row(
                "SELECT val FROM derived WHERE kind=?1 AND ver=?2 AND key=?3",
                rusqlite::params![kind, ver, key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn derived_put(&self, kind: &str, ver: u32, key: &[u8], val: &[u8]) -> Result<()> {
        let guard = self.lock();
        guard.meta.execute(
            "INSERT OR IGNORE INTO derived(kind, ver, key, val) VALUES (?1,?2,?3,?4)",
            rusqlite::params![kind, ver, key, val],
        )?;
        Ok(())
    }

    pub fn record_path(&self, ns: &str, path: &str, seq: i64, cs: ChangesetId) -> Result<()> {
        let guard = self.lock();
        guard.meta.execute(
            "INSERT OR IGNORE INTO path_history(ns, path, seq, changeset) VALUES (?1,?2,?3,?4)",
            rusqlite::params![ns, path, seq, cs.as_bytes().as_slice()],
        )?;
        Ok(())
    }

    pub fn path_history(&self, ns: &str, path: &str, limit: i64) -> Result<Vec<ChangesetId>> {
        let guard = self.lock();
        let mut stmt = guard.meta.prepare(
            "SELECT changeset FROM path_history WHERE ns=?1 AND path=?2 ORDER BY seq DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![ns, path, limit], |r| {
            r.get::<_, Vec<u8>>(0)
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(id_from_vec(&row?));
        }
        Ok(out)
    }

    pub fn put_git_oid(&self, ns: &str, oid: &str, cs: ChangesetId) -> Result<()> {
        let guard = self.lock();
        guard.meta.execute(
            "INSERT OR REPLACE INTO git_map(ns, git_oid, changeset) VALUES (?1,?2,?3)",
            rusqlite::params![ns, oid, cs.as_bytes().as_slice()],
        )?;
        Ok(())
    }

    pub fn get_git_oid(&self, ns: &str, oid: &str) -> Result<Option<ChangesetId>> {
        let guard = self.lock();
        let row: Option<Vec<u8>> = guard
            .meta
            .query_row(
                "SELECT changeset FROM git_map WHERE ns=?1 AND git_oid=?2",
                rusqlite::params![ns, oid],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row.map(|b| id_from_vec(&b)))
    }

    pub fn idem_get(&self, principal: &str, key: &str) -> Result<Option<Vec<u8>>> {
        let guard = self.lock();
        let row: Option<(Vec<u8>, i64)> = guard
            .meta
            .query_row(
                "SELECT response, expires_at FROM idempotency WHERE principal=?1 AND key=?2",
                rusqlite::params![principal, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((body, exp)) if exp > now() => Some(body),
            _ => None,
        })
    }

    pub fn idem_put(
        &self,
        principal: &str,
        key: &str,
        response: &[u8],
        ttl_secs: i64,
    ) -> Result<()> {
        let guard = self.lock();
        guard.meta.execute(
            "INSERT OR REPLACE INTO idempotency(principal, key, response, expires_at) VALUES (?1,?2,?3,?4)",
            rusqlite::params![principal, key, response, now() + ttl_secs],
        )?;
        Ok(())
    }

    pub fn label_domain(&self, blob: &[u8; 32], ns: &str, domain: &str) -> Result<()> {
        let guard = self.lock();
        guard.meta.execute(
            "INSERT OR IGNORE INTO object_domain(blob, ns, domain) VALUES (?1,?2,?3)",
            rusqlite::params![blob.as_slice(), ns, domain],
        )?;
        Ok(())
    }

    pub fn domains_for(&self, blob: &[u8; 32], ns: &str) -> Result<Vec<String>> {
        let guard = self.lock();
        let mut stmt = guard
            .meta
            .prepare("SELECT domain FROM object_domain WHERE blob=?1 AND ns=?2")?;
        let rows = stmt.query_map(rusqlite::params![blob.as_slice(), ns], |r| {
            r.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn record_land_event(
        &self,
        repo_id: i64,
        ref_name: &str,
        changeset: Option<ChangesetId>,
        outcome: &str,
        attempts: i64,
        actor: Option<i64>,
        detail: &str,
    ) -> Result<()> {
        let guard = self.lock();
        guard.meta.execute(
            "INSERT INTO land_events(repo_id, ref, changeset, outcome, attempts, actor_user_id, detail, ts)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                repo_id,
                ref_name,
                changeset.map(|c| c.as_bytes().to_vec()),
                outcome,
                attempts,
                actor,
                detail,
                now()
            ],
        )?;
        Ok(())
    }

    pub fn list_land_events(&self, repo_id: i64, limit: i64) -> Result<Vec<LandEvent>> {
        let guard = self.lock();
        let mut stmt = guard.meta.prepare(
            "SELECT ref, outcome, attempts, COALESCE(detail,''), ts FROM land_events
             WHERE repo_id=?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![repo_id, limit], |r| {
            Ok(LandEvent {
                ref_name: r.get(0)?,
                outcome: r.get(1)?,
                attempts: r.get(2)?,
                detail: r.get(3)?,
                ts: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn audit(
        &self,
        actor: Option<i64>,
        via: &str,
        action: &str,
        repo_id: Option<i64>,
        target: &str,
        outcome: &str,
        detail: &str,
    ) -> Result<()> {
        let guard = self.lock();
        guard.meta.execute(
            "INSERT INTO audit(ts, actor_user_id, actor_via, action, repo_id, target, outcome, detail)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![now(), actor, via, action, repo_id, target, outcome, detail],
        )?;
        Ok(())
    }

    pub fn doctor(&self) -> Result<DoctorReport> {
        let guard = self.lock();
        let changesets: i64 = guard
            .meta
            .query_row("SELECT COUNT(*) FROM changesets", [], |r| r.get(0))?;
        let refs: i64 = guard
            .meta
            .query_row("SELECT COUNT(*) FROM refs", [], |r| r.get(0))?;
        let blobs: i64 = guard
            .meta
            .query_row("SELECT COUNT(*) FROM blob_index", [], |r| r.get(0))?;
        let mut problems = Vec::new();
        let mut stmt = guard.meta.prepare("SELECT ns, name, target FROM refs")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        for row in rows {
            let (ns, name, target) = row?;
            let n: i64 = guard.meta.query_row(
                "SELECT COUNT(*) FROM changesets WHERE ns=?1 AND id=?2",
                rusqlite::params![ns, target],
                |r| r.get(0),
            )?;
            if n == 0 {
                problems.push(format!("ref {ns}:{name} points at a missing changeset"));
            }
        }
        drop(stmt);
        Ok(DoctorReport {
            changesets,
            refs,
            blobs,
            problems,
        })
    }

    /// Delete unreachable blobs and abandoned changesets older than `grace_secs`.
    /// Reachable means referenced from a ref, walking first parents, plus blobs
    /// named by those changesets. Never deletes a blob that is still indexed as
    /// reachable.
    pub fn gc(&self, grace_secs: i64) -> Result<GcReport> {
        let guard = self.lock();
        let cutoff = now() - grace_secs;
        let mut reachable_cs: Vec<(String, Vec<u8>)> = Vec::new();
        let mut stmt = guard.meta.prepare("SELECT ns, target FROM refs")?;
        let seeds: Vec<(String, Vec<u8>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut stack = seeds;
        let mut seen = std::collections::BTreeSet::new();
        while let Some((ns, id)) = stack.pop() {
            if !seen.insert((ns.clone(), id.clone())) {
                continue;
            }
            reachable_cs.push((ns.clone(), id.clone()));
            if let Some(parents) = guard
                .meta
                .query_row(
                    "SELECT parents FROM changesets WHERE ns=?1 AND id=?2",
                    rusqlite::params![ns, id],
                    |r| r.get::<_, Vec<u8>>(0),
                )
                .optional()?
            {
                for chunk in parents.chunks(32) {
                    if chunk.len() == 32 {
                        stack.push((ns.clone(), chunk.to_vec()));
                    }
                }
            }
        }
        let mut keep_blobs = std::collections::BTreeSet::new();
        for (ns, id) in &reachable_cs {
            if let Some(body) = guard
                .meta
                .query_row(
                    "SELECT body FROM changesets WHERE ns=?1 AND id=?2",
                    rusqlite::params![ns, id],
                    |r| r.get::<_, Vec<u8>>(0),
                )
                .optional()?
            {
                if let Ok(plain) = zstd::stream::decode_all(std::io::Cursor::new(body)) {
                    if let Ok(cs) = Changeset::decode(&plain) {
                        for change in cs.changes.values() {
                            if let scion_core::Change::Put { blob, .. } = change {
                                keep_blobs.insert(*blob.as_bytes());
                            }
                        }
                    }
                }
            }
        }
        let mut stmt = guard
            .meta
            .prepare("SELECT id, created_at FROM blob_index")?;
        let blobs: Vec<(Vec<u8>, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        let mut deleted = 0u64;
        for (id, created) in blobs {
            if id.len() != 32 || created > cutoff {
                continue;
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&id);
            if keep_blobs.contains(&key) {
                continue;
            }
            let live_chunk = chunk_of_live_blob(&guard.meta, &id, &keep_blobs)?;
            if live_chunk {
                continue;
            }
            guard
                .meta
                .execute("DELETE FROM blob_index WHERE id=?1", [id.as_slice()])?;
            guard
                .meta
                .execute("DELETE FROM blob_manifest WHERE id=?1", [id.as_slice()])?;
            guard
                .small
                .execute("DELETE FROM small_blobs WHERE id=?1", [id.as_slice()])?;
            let _ = fs::remove_file(self.root.join(blob_rel("blobs", &key)));
            let _ = fs::remove_file(self.root.join(blob_rel("chunks", &key)));
            deleted += 1;
        }
        let mut cs_deleted = 0u64;
        let mut stmt = guard
            .meta
            .prepare("SELECT ns, id, created_at FROM changesets")?;
        let all: Vec<(String, Vec<u8>, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        for (ns, id, created) in all {
            if created > cutoff {
                continue;
            }
            if reachable_cs.iter().any(|(n, i)| n == &ns && i == &id) {
                continue;
            }
            guard.meta.execute(
                "DELETE FROM changesets WHERE ns=?1 AND id=?2",
                rusqlite::params![ns, id],
            )?;
            cs_deleted += 1;
        }
        Ok(GcReport {
            blobs_deleted: deleted,
            changesets_deleted: cs_deleted,
        })
    }

    pub fn backup_sqlite(&self, meta_dest: &Path, small_dest: &Path) -> Result<()> {
        if let Some(parent) = meta_dest.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Some(parent) = small_dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let guard = self.lock();
        backup_conn(&guard.meta, meta_dest)?;
        backup_conn(&guard.small, small_dest)?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<String> {
        let guard = self.lock();
        let v: String = guard.meta.query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |r| r.get(0),
        )?;
        Ok(v)
    }
}

fn backup_conn(src: &Connection, dest: &Path) -> Result<()> {
    if dest.exists() {
        fs::remove_file(dest)?;
    }
    let mut dst = Connection::open(dest)?;
    let backup = rusqlite::backup::Backup::new(src, &mut dst)?;
    backup.step(-1)?;
    Ok(())
}

fn open_sqlite(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

fn id_from_vec(bytes: &[u8]) -> ChangesetId {
    let mut id = [0u8; 32];
    let n = bytes.len().min(32);
    id[..n].copy_from_slice(&bytes[..n]);
    ChangesetId::from_bytes(id)
}

fn chunk_of_live_blob(
    meta: &Connection,
    chunk: &[u8],
    keep_blobs: &std::collections::BTreeSet<[u8; 32]>,
) -> Result<bool> {
    let mut stmt = meta.prepare("SELECT id FROM blob_manifest WHERE chunk=?1")?;
    let parents: Vec<Vec<u8>> = stmt
        .query_map([chunk], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for parent in parents {
        if parent.len() != 32 {
            continue;
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&parent);
        if keep_blobs.contains(&key) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn blob_rel(kind: &str, id: &[u8; 32]) -> PathBuf {
    let h = hex::encode(id);
    PathBuf::from(kind)
        .join(&h[..2])
        .join(&h[2..4])
        .join(&h[4..])
}
