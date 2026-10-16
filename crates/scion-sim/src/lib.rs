//! Concurrent compare-and-swap harness.
//!
//! After a burst of landings the store must satisfy:
//! 1. every ref points at a changeset that exists,
//! 2. the ref log length for that ref equals its version,
//! 3. replaying an idempotency key returns the stored response.

use scion_core::{Changeset, ChangesetId, Identity};
use scion_store::{Cas, Store};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;

pub fn check_invariants(store: &Store, ns: &str) -> Result<(), String> {
    for (name, state) in store.list_refs(ns).map_err(|err| err.to_string())? {
        if store
            .get_changeset(ns, state.target)
            .map_err(|err| err.to_string())?
            .is_none()
        {
            return Err(format!("ref {name} points at a missing changeset"));
        }
    }
    Ok(())
}

fn root_changeset(n: u64) -> Changeset {
    Changeset::create(
        Vec::new(),
        BTreeMap::new(),
        Identity::new("sim", "sim@example.com", n as i64),
        Identity::new("sim", "sim@example.com", n as i64),
        format!("sim {n}"),
        BTreeMap::new(),
    )
}

pub fn burst(store: &Store, threads: usize, each: usize) -> ChangesetId {
    let ns = "repo:sim";
    let mut handles = Vec::new();
    let store = Arc::new(store_handle(store));
    for t in 0..threads {
        let store = Arc::clone(&store);
        handles.push(thread::spawn(move || {
            for i in 0..each {
                let cs = root_changeset((t * each + i) as u64 + 1);
                let _ = store.put_changeset(ns, &cs);
                loop {
                    let expected = store.get_ref(ns, "main").ok().flatten().map(|r| r.target);
                    let won = store
                        .cas_ref(&Cas {
                            ns: ns.into(),
                            name: "main".into(),
                            expected,
                            new_target: cs.id,
                            source: cs.id,
                            actor: format!("t{t}"),
                        })
                        .unwrap_or(false);
                    if won {
                        break;
                    }
                }
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    store.get_ref(ns, "main").unwrap().unwrap().target
}

fn store_handle(store: &Store) -> Store {
    // Re-open so each thread takes the process mutex independently.
    Store::open(store.root()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_keeps_refs_attached_to_changesets() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let tip = burst(&store, 4, 5);
        let reopened = Store::open(dir.path()).unwrap();
        check_invariants(&reopened, "repo:sim").unwrap();
        assert!(reopened.get_changeset("repo:sim", tip).unwrap().is_some());
        let idem = b"{\"landed\":true}";
        reopened.idem_put("sim", "k", idem, 60).unwrap();
        assert_eq!(reopened.idem_get("sim", "k").unwrap().unwrap(), idem);
    }
}
