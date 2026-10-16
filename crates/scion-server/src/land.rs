//! Rebase-or-fast-forward landing. The ref move is one compare-and-swap.

use crate::app::App;
use scion_core::{BlobId, Change, Changeset, ChangesetId, FileKind, Identity, RepoPath};
use scion_merge::{rebase_changes, NodeState, PathAction};
use scion_store::Cas;
use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    FastForwardOnly,
    Rebase,
    Merge,
}

impl Strategy {
    pub fn parse(s: &str) -> Self {
        match s {
            "fast_forward" | "fast_forward_only" => Strategy::FastForwardOnly,
            "merge" => Strategy::Merge,
            _ => Strategy::Rebase,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LandOutcome {
    pub landed: ChangesetId,
    pub rebased_from: Option<ChangesetId>,
    pub attempts: u32,
}

#[derive(Debug)]
pub enum LandError {
    NotFound,
    NotFastForward,
    Conflict(scion_merge::Conflicts),
    Contended { attempts: u32 },
    Store(scion_store::Error),
}

impl From<scion_store::Error> for LandError {
    fn from(e: scion_store::Error) -> Self {
        LandError::Store(e)
    }
}

pub struct LandRequest {
    pub ns: String,
    pub repo_id: i64,
    pub ref_name: String,
    pub changeset: ChangesetId,
    pub strategy: Strategy,
    pub actor: String,
    pub actor_id: Option<i64>,
    pub max_attempts: u32,
}

pub fn land(app: &App, req: &LandRequest) -> Result<LandOutcome, LandError> {
    let original = app
        .store
        .get_changeset(&req.ns, req.changeset)?
        .ok_or(LandError::NotFound)?;
    let mut cand = original.clone();
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let tip = app.store.get_ref(&req.ns, &req.ref_name)?;
        if let Some(tip) = &tip {
            if cand.first_parent() != Some(tip.target) {
                match req.strategy {
                    Strategy::FastForwardOnly => {
                        app.store.record_land_event(
                            req.repo_id,
                            &req.ref_name,
                            Some(req.changeset),
                            "contended",
                            attempts as i64,
                            req.actor_id,
                            "not a fast-forward",
                        )?;
                        return Err(LandError::NotFastForward);
                    }
                    Strategy::Rebase | Strategy::Merge => {
                        cand = rebase_onto(app, &req.ns, &cand, tip.target, req.strategy)?;
                        app.store.put_changeset(&req.ns, &cand)?;
                    }
                }
            }
        }
        app.engine
            .ensure_eager(&req.ns, cand.id)
            .map_err(LandError::from)?;
        let expected = tip.map(|t| t.target);
        let won = app.store.cas_ref(&Cas {
            ns: req.ns.clone(),
            name: req.ref_name.clone(),
            expected,
            new_target: cand.id,
            source: req.changeset,
            actor: req.actor.clone(),
        })?;
        if won {
            app.store.record_land_event(
                req.repo_id,
                &req.ref_name,
                Some(req.changeset),
                "landed",
                attempts as i64,
                req.actor_id,
                &cand.id.to_hex(),
            )?;
            return Ok(LandOutcome {
                landed: cand.id,
                rebased_from: (cand.id != req.changeset).then_some(req.changeset),
                attempts,
            });
        }
        if attempts >= req.max_attempts {
            app.store.record_land_event(
                req.repo_id,
                &req.ref_name,
                Some(req.changeset),
                "contended",
                attempts as i64,
                req.actor_id,
                "cas lost",
            )?;
            return Err(LandError::Contended { attempts });
        }
        thread::sleep(Duration::from_millis(2 + (attempts as u64 % 5)));
        // Retry from the original so each attempt rebases onto the latest tip.
        cand = original.clone();
    }
}

fn rebase_onto(
    app: &App,
    ns: &str,
    cs: &Changeset,
    onto: ChangesetId,
    strategy: Strategy,
) -> Result<Changeset, LandError> {
    let parent = cs.first_parent();
    let read = |blob: BlobId| -> Option<String> {
        let bytes = app.store.get_blob(blob).ok()?;
        if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            None
        } else {
            Some(String::from_utf8(bytes).ok()?)
        }
    };
    let state = |id: Option<ChangesetId>, path: &RepoPath| -> NodeState {
        let Some(id) = id else {
            return NodeState::Absent;
        };
        app.engine
            .node_at(ns, id, path)
            .unwrap_or(NodeState::Absent)
    };
    let actions = rebase_changes(
        &cs.changes,
        |path| state(parent, path),
        |path| state(Some(onto), path),
        read,
    )
    .map_err(|c| {
        let _ = app.store.record_land_event(
            0,
            "",
            Some(cs.id),
            "conflict",
            1,
            None,
            &c.items
                .iter()
                .map(|i| i.path.to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
        LandError::Conflict(c)
    })?;
    let mut changes = BTreeMap::new();
    for (path, action) in actions {
        match action {
            PathAction::Drop => {}
            PathAction::Keep(change) => {
                changes.insert(path, change);
            }
            PathAction::Merged { kind, text } => {
                let bytes = text.into_bytes();
                let id = BlobId::of(&bytes);
                app.store.put_blob(id, &bytes)?;
                changes.insert(
                    path,
                    Change::Put {
                        kind,
                        blob: id,
                        size: bytes.len() as u64,
                    },
                );
            }
        }
    }
    let mut extra = cs.extra.clone();
    extra.insert("rebased-from".into(), cs.id.to_hex().into_bytes());
    let parents = match strategy {
        Strategy::Merge => vec![onto, cs.id],
        _ => vec![onto],
    };
    Ok(Changeset::create(
        parents,
        changes,
        cs.author.clone(),
        cs.committer.clone(),
        cs.message.clone(),
        extra,
    ))
}

pub fn identity_for(name: &str) -> Identity {
    let when = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Identity::new(name, format!("{name}@scion.local"), when)
}

#[allow(dead_code)]
pub fn put_from_text(kind: FileKind, text: &str) -> (Change, Vec<u8>) {
    let bytes = text.as_bytes().to_vec();
    let id = BlobId::of(&bytes);
    (
        Change::Put {
            kind,
            blob: id,
            size: bytes.len() as u64,
        },
        bytes,
    )
}
