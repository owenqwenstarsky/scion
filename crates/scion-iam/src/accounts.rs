use crate::authz::{decide, narrow, AuthzError, Principal, Role, Via};
use crate::password::{self, PasswordParams};
use crate::Error;
use globset::GlobBuilder;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use scion_core::{domains, hash_token};
use scion_store::Store;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const IDLE_SECS: i64 = 12 * 3600;
const ABSOLUTE_SECS: i64 = 7 * 86400;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn random_token() -> (String, [u8; 32]) {
    let mut raw = [0u8; 32];
    let mut file = std::fs::File::open("/dev/urandom").expect("urandom");
    std::io::Read::read_exact(&mut file, &mut raw).expect("urandom");
    (hex::encode(raw), raw)
}

fn session_hash(raw: &[u8]) -> [u8; 32] {
    hash_token(domains::SESSION, raw).0
}

fn api_hash(raw: &[u8]) -> [u8; 32] {
    hash_token(domains::API_TOKEN, raw).0
}

fn valid_slug(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 39 || !b[0].is_ascii_alphabetic() {
        return false;
    }
    b.iter()
        .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
}

#[derive(Clone, Debug)]
pub struct LoginResult {
    pub session_token: String,
    pub csrf_token: String,
    pub must_change_password: bool,
    pub user_id: i64,
    pub username: String,
    pub site_admin: bool,
}

#[derive(Clone, Debug)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub user_kind: String,
    pub site_admin: bool,
    pub status: String,
    pub must_change_password: bool,
    pub locked_until: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct TokenInfo {
    pub id: i64,
    pub name: String,
    pub prefix: String,
    pub max_role: String,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct RepoRow {
    pub id: i64,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub default_ref: String,
    pub role: Role,
}

pub struct Accounts {
    store: Arc<Store>,
    params: PasswordParams,
    dummy_hash: String,
}

impl Accounts {
    pub fn new(store: Arc<Store>, params: PasswordParams) -> Result<Self, Error> {
        let dummy_hash =
            password::hash_password(&format!("not-a-real-{}", hex::encode([7u8; 16])), &params)?;
        Ok(Self {
            store,
            params,
            dummy_hash,
        })
    }

    fn conn<T>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> Result<T, Error> {
        self.store
            .with_meta(f)
            .map_err(scion_store::Error::from)
            .map_err(Error::from)
    }

    pub fn user_count(&self) -> Result<i64, Error> {
        self.conn(|c| c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)))
    }

    pub fn create_first_admin(&self, username: &str, password: &str) -> Result<i64, Error> {
        if self.user_count()? > 0 {
            return Err(Error::AlreadyBootstrapped);
        }
        self.insert_user(username, password, "human", true, true)
    }

    pub fn create_user(
        &self,
        username: &str,
        password: Option<&str>,
        kind: &str,
        site_admin: bool,
    ) -> Result<(i64, Option<String>), Error> {
        if kind != "human" {
            return self
                .insert_user(username, "", kind, site_admin, false)
                .map(|id| (id, None));
        }
        match password {
            Some(pw) => self
                .insert_user(username, pw, kind, site_admin, false)
                .map(|id| (id, None)),
            None => self.create_user_generated(username, kind, site_admin),
        }
    }

    fn create_user_generated(
        &self,
        username: &str,
        kind: &str,
        site_admin: bool,
    ) -> Result<(i64, Option<String>), Error> {
        let (_, raw) = random_token();
        let password = format!("Tmp-{}", hex::encode(raw));
        let id = self.insert_user(username, &password, kind, site_admin, true)?;
        Ok((id, Some(password)))
    }

    fn insert_user(
        &self,
        username: &str,
        password: &str,
        kind: &str,
        site_admin: bool,
        must_change: bool,
    ) -> Result<i64, Error> {
        if !valid_slug(username) {
            return Err(Error::Core(scion_core::Error::Invalid(
                "invalid username".into(),
            )));
        }
        if !matches!(kind, "human" | "agent" | "service") {
            return Err(Error::Core(scion_core::Error::Invalid(
                "invalid user kind".into(),
            )));
        }
        let hash = if kind == "human" {
            Some(password::hash_password(password, &self.params)?)
        } else {
            None
        };
        self.store
            .with_meta_mut(|c| -> rusqlite::Result<i64> {
                let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute(
                    "INSERT INTO accounts(slug, kind, display_name, created_at) VALUES (?1,'user',?1,?2)",
                    params![username, now()],
                )?;
                let id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO users(id, user_kind, password_hash, is_site_admin, must_change_password, password_changed_at)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    params![id, kind, hash, site_admin as i64, must_change as i64, now()],
                )?;
                tx.commit()?;
                Ok(id)
            })
            .map_err(scion_store::Error::from)
            .map_err(Error::from)
    }

    pub fn list_users(&self) -> Result<Vec<UserRow>, Error> {
        self.conn(|c| {
            let mut stmt = c.prepare(
                "SELECT u.id, a.slug, u.user_kind, u.is_site_admin, u.status, u.must_change_password, u.locked_until
                 FROM users u JOIN accounts a ON a.id = u.id ORDER BY a.slug",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(UserRow {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    user_kind: r.get(2)?,
                    site_admin: r.get::<_, i64>(3)? != 0,
                    status: r.get(4)?,
                    must_change_password: r.get::<_, i64>(5)? != 0,
                    locked_until: r.get(6)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn set_status(&self, id: i64, status: &str) -> Result<(), Error> {
        self.conn(|c| {
            c.execute(
                "UPDATE users SET status=?1 WHERE id=?2",
                params![status, id],
            )?;
            if status != "active" {
                c.execute(
                    "UPDATE sessions SET revoked_at=?1 WHERE user_id=?2 AND revoked_at IS NULL",
                    params![now(), id],
                )?;
            }
            Ok(())
        })
    }

    pub fn set_site_admin(&self, id: i64, admin: bool) -> Result<(), Error> {
        self.conn(|c| {
            c.execute(
                "UPDATE users SET is_site_admin=?1 WHERE id=?2",
                params![admin as i64, id],
            )?;
            Ok(())
        })
    }

    pub fn unlock(&self, id: i64) -> Result<(), Error> {
        self.conn(|c| {
            c.execute(
                "UPDATE users SET failed_logins=0, locked_until=NULL WHERE id=?1",
                [id],
            )?;
            Ok(())
        })
    }

    pub fn reset_password(&self, id: i64) -> Result<String, Error> {
        let (_, raw) = random_token();
        let password = format!("Tmp-{}", hex::encode(raw));
        let hash = password::hash_password(&password, &self.params)?;
        self.conn(|c| {
            c.execute(
                "UPDATE users SET password_hash=?1, must_change_password=1, failed_logins=0, locked_until=NULL, password_changed_at=?2 WHERE id=?3",
                params![hash, now(), id],
            )?;
            c.execute(
                "UPDATE sessions SET revoked_at=?1 WHERE user_id=?2 AND revoked_at IS NULL",
                params![now(), id],
            )?;
            Ok(())
        })?;
        Ok(password)
    }

    pub fn login(
        &self,
        username: &str,
        password: &str,
        source: &str,
    ) -> Result<LoginResult, Error> {
        let row = self.conn(|c| {
            c.query_row(
                "SELECT u.id, u.password_hash, u.status, u.must_change_password, u.is_site_admin, u.locked_until, u.failed_logins, u.user_kind
                 FROM users u JOIN accounts a ON a.id = u.id WHERE a.slug=?1 COLLATE NOCASE",
                [username],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()
        })?;
        let Some((id, hash, status, must, admin, locked, fails, kind)) = row else {
            let _ = password::verify_password(password, &self.dummy_hash);
            self.record_attempt(username, source, false)?;
            return Err(Error::InvalidCredentials);
        };
        if kind != "human" || status != "active" {
            let _ = password::verify_password(password, &self.dummy_hash);
            self.record_attempt(username, source, false)?;
            return Err(Error::InvalidCredentials);
        }
        if locked.unwrap_or(0) > now() {
            let _ =
                password::verify_password(password, hash.as_deref().unwrap_or(&self.dummy_hash));
            self.record_attempt(username, source, false)?;
            return Err(Error::InvalidCredentials);
        }
        let ok = hash
            .as_deref()
            .map(|h| password::verify_password(password, h))
            .unwrap_or(false);
        if !ok {
            let fails = fails + 1;
            let lock = if fails >= 5 {
                let shift = (fails - 5).min(10);
                Some(now() + 60 * (1i64 << shift))
            } else {
                None
            };
            self.conn(|c| {
                c.execute(
                    "UPDATE users SET failed_logins=?1, locked_until=?2 WHERE id=?3",
                    params![fails, lock, id],
                )?;
                Ok(())
            })?;
            self.record_attempt(username, source, false)?;
            return Err(Error::InvalidCredentials);
        }
        if let Some(h) = &hash {
            if password::needs_rehash(h, &self.params) {
                if let Ok(new_hash) = password::hash_password(password, &self.params) {
                    let _ = self.conn(|c| {
                        c.execute(
                            "UPDATE users SET password_hash=?1 WHERE id=?2",
                            params![new_hash, id],
                        )?;
                        Ok(())
                    });
                }
            }
        }
        self.conn(|c| {
            c.execute(
                "UPDATE users SET failed_logins=0, locked_until=NULL, last_login_at=?1 WHERE id=?2",
                params![now(), id],
            )?;
            Ok(())
        })?;
        self.record_attempt(username, source, true)?;
        let session = self.issue_session(id)?;
        Ok(LoginResult {
            session_token: session.0,
            csrf_token: session.1,
            must_change_password: must != 0,
            user_id: id,
            username: username.to_string(),
            site_admin: admin != 0,
        })
    }

    fn record_attempt(&self, username: &str, source: &str, success: bool) -> Result<(), Error> {
        self.conn(|c| {
            c.execute(
                "INSERT INTO login_attempts(ts, username_norm, source, success) VALUES (?1,?2,?3,?4)",
                params![now(), username.to_ascii_lowercase(), source, success as i64],
            )?;
            Ok(())
        })
    }

    fn issue_session(&self, user_id: i64) -> Result<(String, String), Error> {
        let (token, raw) = random_token();
        let (csrf, csrf_raw) = random_token();
        let id = session_hash(&raw);
        let ts = now();
        self.conn(|c| {
            c.execute(
                "INSERT INTO sessions(id, user_id, csrf_secret, created_at, last_seen_at, idle_expires_at, absolute_expires_at)
                 VALUES (?1,?2,?3,?4,?4,?5,?6)",
                params![id.as_slice(), user_id, csrf_raw.as_slice(), ts, ts + IDLE_SECS, ts + ABSOLUTE_SECS],
            )?;
            Ok(())
        })?;
        Ok((token, csrf))
    }

    pub fn logout(&self, session_token: &str) -> Result<(), Error> {
        let raw = hex::decode(session_token).unwrap_or_default();
        let id = session_hash(&raw);
        self.conn(|c| {
            c.execute(
                "UPDATE sessions SET revoked_at=?1 WHERE id=?2 AND revoked_at IS NULL",
                params![now(), id.as_slice()],
            )?;
            Ok(())
        })
    }

    pub fn revoke_all_sessions(&self, user_id: i64) -> Result<(), Error> {
        self.conn(|c| {
            c.execute(
                "UPDATE sessions SET revoked_at=?1 WHERE user_id=?2 AND revoked_at IS NULL",
                params![now(), user_id],
            )?;
            Ok(())
        })
    }

    pub fn authenticate_session(&self, session_token: &str) -> Result<Option<Principal>, Error> {
        let raw = match hex::decode(session_token) {
            Ok(b) if b.len() == 32 => b,
            _ => return Ok(None),
        };
        let id = session_hash(&raw);
        let row = self.conn(|c| {
            c.query_row(
                "SELECT s.user_id, s.csrf_secret, s.idle_expires_at, s.absolute_expires_at, s.revoked_at,
                        a.slug, u.is_site_admin, u.status
                 FROM sessions s JOIN users u ON u.id = s.user_id JOIN accounts a ON a.id = u.id
                 WHERE s.id=?1",
                [id.as_slice()],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Vec<u8>>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()
        })?;
        let Some((user_id, csrf, idle, absolute, revoked, username, admin, status)) = row else {
            return Ok(None);
        };
        let ts = now();
        if revoked.is_some() || status != "active" || ts > idle || ts > absolute {
            return Ok(None);
        }
        let new_idle = (ts + IDLE_SECS).min(absolute);
        let _ = self.conn(|c| {
            c.execute(
                "UPDATE sessions SET last_seen_at=?1, idle_expires_at=?2 WHERE id=?3",
                params![ts, new_idle, id.as_slice()],
            )?;
            Ok(())
        });
        Ok(Some(Principal {
            user_id: Some(user_id),
            username,
            site_admin: admin != 0,
            via: Via::Session,
            session_hash: Some(id),
            csrf: Some(hex::encode(csrf)),
        }))
    }

    pub fn csrf_matches(principal: &Principal, presented: &str) -> bool {
        match &principal.csrf {
            Some(secret) => {
                if secret.len() != presented.len() {
                    return false;
                }
                subtle::ConstantTimeEq::ct_eq(secret.as_bytes(), presented.as_bytes()).into()
            }
            None => false,
        }
    }

    pub fn change_password(
        &self,
        user_id: i64,
        current: &str,
        new_password: &str,
    ) -> Result<(), Error> {
        let hash = self.conn(|c| {
            c.query_row(
                "SELECT password_hash FROM users WHERE id=?1",
                [user_id],
                |r| r.get::<_, Option<String>>(0),
            )
        })?;
        let Some(hash) = hash else {
            return Err(Error::InvalidCredentials);
        };
        if !password::verify_password(current, &hash) {
            return Err(Error::InvalidCredentials);
        }
        let new_hash = password::hash_password(new_password, &self.params)?;
        self.conn(|c| {
            c.execute(
                "UPDATE users SET password_hash=?1, must_change_password=0, password_changed_at=?2 WHERE id=?3",
                params![new_hash, now(), user_id],
            )?;
            c.execute(
                "UPDATE sessions SET revoked_at=?1 WHERE user_id=?2 AND revoked_at IS NULL",
                params![now(), user_id],
            )?;
            Ok(())
        })
    }

    pub fn create_repo(&self, owner_id: i64, name: &str, description: &str) -> Result<i64, Error> {
        if !valid_slug(name) {
            return Err(Error::Core(scion_core::Error::Invalid(
                "invalid repository name".into(),
            )));
        }
        self.conn(|c| {
            c.execute(
                "INSERT INTO repos(owner_id, name, description, created_at) VALUES (?1,?2,?3,?4)",
                params![owner_id, name, description, now()],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn repo_by_name(
        &self,
        owner: &str,
        name: &str,
    ) -> Result<Option<(i64, i64, String)>, Error> {
        self.conn(|c| {
            c.query_row(
                "SELECT r.id, r.owner_id, r.default_ref FROM repos r
                 JOIN accounts a ON a.id = r.owner_id
                 WHERE a.slug=?1 COLLATE NOCASE AND r.name=?2 COLLATE NOCASE AND r.deleted_at IS NULL",
                params![owner, name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })
    }

    pub fn list_repos(&self, user_id: i64) -> Result<Vec<RepoRow>, Error> {
        let ids = self.conn(|c| {
            let mut stmt = c.prepare(
                "SELECT r.id FROM repos r WHERE r.deleted_at IS NULL AND (
                    r.owner_id = ?1
                    OR r.id IN (SELECT repo_id FROM repo_grants WHERE principal_type='user' AND principal_id=?1)
                    OR r.owner_id IN (SELECT org_id FROM org_members WHERE user_id=?1)
                )",
            )?;
            let rows = stmt.query_map([user_id], |r| r.get::<_, i64>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })?;
        let mut out = Vec::new();
        for id in ids {
            let role = self.effective_role(user_id, id)?;
            if role == Role::None {
                continue;
            }
            if let Some(row) = self.repo_row(id, role)? {
                out.push(row);
            }
        }
        Ok(out)
    }

    fn repo_row(&self, id: i64, role: Role) -> Result<Option<RepoRow>, Error> {
        self.conn(|c| {
            c.query_row(
                "SELECT a.slug, r.name, r.description, r.default_ref FROM repos r
                 JOIN accounts a ON a.id = r.owner_id WHERE r.id=?1",
                [id],
                |r| {
                    Ok(RepoRow {
                        id,
                        owner: r.get(0)?,
                        name: r.get(1)?,
                        description: r.get(2)?,
                        default_ref: r.get(3)?,
                        role,
                    })
                },
            )
            .optional()
        })
    }

    pub fn effective_role(&self, user_id: i64, repo_id: i64) -> Result<Role, Error> {
        let row = self.conn(|c| {
            c.query_row(
                "SELECT r.owner_id, a.kind FROM repos r JOIN accounts a ON a.id = r.owner_id
                 WHERE r.id=?1 AND r.deleted_at IS NULL",
                [repo_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
        })?;
        let Some((owner_id, kind)) = row else {
            return Ok(Role::None);
        };
        let mut role = Role::None;
        if kind == "user" && owner_id == user_id {
            role = Role::Owner;
        }
        if kind == "org" {
            let membership = self.conn(|c| {
                c.query_row(
                    "SELECT m.role, o.default_repo_role FROM org_members m JOIN orgs o ON o.id = m.org_id
                     WHERE m.org_id=?1 AND m.user_id=?2",
                    params![owner_id, user_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()
            })?;
            if let Some((mrole, default_role)) = membership {
                let mapped = match mrole.as_str() {
                    "owner" => Role::Owner,
                    "admin" => Role::Admin,
                    "member" => Role::parse(&default_role).unwrap_or(Role::None),
                    _ => Role::None,
                };
                role = role.max(mapped);
            }
        }
        let grant = self.conn(|c| {
            c.query_row(
                "SELECT role FROM repo_grants WHERE repo_id=?1 AND principal_type='user' AND principal_id=?2",
                params![repo_id, user_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })?;
        if let Some(g) = grant {
            if let Some(parsed) = Role::parse(&g) {
                role = role.max(parsed);
            }
        }
        Ok(role)
    }

    pub fn role_for(&self, principal: &Principal, repo_id: i64) -> Result<Role, Error> {
        let Some(user) = principal.user_id else {
            return Ok(Role::None);
        };
        let role = self.effective_role(user, repo_id)?;
        Ok(narrow(role, &principal.via, repo_id))
    }

    pub fn authorize(
        &self,
        principal: &Principal,
        repo_id: i64,
        action: crate::authz::Action,
        path: Option<&str>,
    ) -> Result<(), AuthzError> {
        if principal.user_id.is_none() {
            return Err(AuthzError::Unauthenticated);
        }
        let role = self
            .role_for(principal, repo_id)
            .map_err(|_| AuthzError::Forbidden)?;
        let denied = if let Some(path) = path {
            self.path_denied(
                principal.user_id.unwrap(),
                repo_id,
                path,
                action.min_role() >= Role::Write,
            )
            .unwrap_or(false)
        } else {
            false
        };
        decide(role, action, denied)
    }

    pub fn path_denied(
        &self,
        user_id: i64,
        repo_id: i64,
        path: &str,
        write: bool,
    ) -> Result<bool, Error> {
        let rules = self.conn(|c| {
            let mut stmt = c.prepare(
                "SELECT path_glob, principal_id, can_read, can_write FROM repo_path_rules WHERE repo_id=?1",
            )?;
            let rows = stmt.query_map([repo_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })?;
        for (glob, principal, can_read, can_write) in rules {
            if let Some(pid) = principal {
                if pid != user_id {
                    continue;
                }
            }
            if !glob_match(&glob, path) {
                continue;
            }
            if write && can_write == 0 {
                return Ok(true);
            }
            if !write && can_read == 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn set_grant(
        &self,
        repo_id: i64,
        user_id: i64,
        role: Role,
        granted_by: i64,
    ) -> Result<(), Error> {
        if role == Role::Owner || role == Role::None {
            return Err(Error::Core(scion_core::Error::Invalid(
                "grant role must be read..admin".into(),
            )));
        }
        self.conn(|c| {
            c.execute(
                "INSERT INTO repo_grants(repo_id, principal_type, principal_id, role, granted_by, created_at)
                 VALUES (?1,'user',?2,?3,?4,?5)
                 ON CONFLICT(repo_id, principal_type, principal_id)
                 DO UPDATE SET role=excluded.role",
                params![repo_id, user_id, role.as_str(), granted_by, now()],
            )?;
            Ok(())
        })
    }

    pub fn remove_grant(&self, repo_id: i64, user_id: i64) -> Result<(), Error> {
        self.conn(|c| {
            c.execute(
                "DELETE FROM repo_grants WHERE repo_id=?1 AND principal_type='user' AND principal_id=?2",
                params![repo_id, user_id],
            )?;
            Ok(())
        })
    }

    pub fn list_grants(&self, repo_id: i64) -> Result<Vec<(i64, String, String)>, Error> {
        self.conn(|c| {
            let mut stmt = c.prepare(
                "SELECT g.principal_id, a.slug, g.role FROM repo_grants g
                 JOIN accounts a ON a.id = g.principal_id
                 WHERE g.repo_id=?1 AND g.principal_type='user'",
            )?;
            let rows = stmt.query_map([repo_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
    }

    pub fn set_path_rule(
        &self,
        repo_id: i64,
        glob: &str,
        principal: Option<i64>,
        can_read: bool,
        can_write: bool,
        created_by: i64,
    ) -> Result<i64, Error> {
        self.conn(|c| {
            c.execute(
                "INSERT INTO repo_path_rules(repo_id, path_glob, principal_type, principal_id, can_read, can_write, created_by, created_at)
                 VALUES (?1,?2,'user',?3,?4,?5,?6,?7)",
                params![repo_id, glob, principal, can_read as i64, can_write as i64, created_by, now()],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn list_path_rules(&self, repo_id: i64) -> Result<Vec<(i64, String, bool, bool)>, Error> {
        self.conn(|c| {
            let mut stmt = c.prepare(
                "SELECT id, path_glob, can_read, can_write FROM repo_path_rules WHERE repo_id=?1",
            )?;
            let rows = stmt.query_map([repo_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2)? != 0,
                    r.get::<_, i64>(3)? != 0,
                ))
            })?;
            rows.collect()
        })
    }

    pub fn create_org(
        &self,
        owner_user: i64,
        slug: &str,
        default_role: &str,
    ) -> Result<i64, Error> {
        if !valid_slug(slug) {
            return Err(Error::Core(scion_core::Error::Invalid(
                "invalid org slug".into(),
            )));
        }
        let default_role = if matches!(default_role, "none" | "read" | "write") {
            default_role
        } else {
            "none"
        };
        self.store
            .with_meta_mut(|c| -> rusqlite::Result<i64> {
                let tx = c.transaction()?;
                tx.execute(
                    "INSERT INTO accounts(slug, kind, display_name, created_at) VALUES (?1,'org',?1,?2)",
                    params![slug, now()],
                )?;
                let id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO orgs(id, default_repo_role) VALUES (?1,?2)",
                    params![id, default_role],
                )?;
                tx.execute(
                    "INSERT INTO org_members(org_id, user_id, role) VALUES (?1,?2,'owner')",
                    params![id, owner_user],
                )?;
                tx.commit()?;
                Ok(id)
            })
            .map_err(scion_store::Error::from)
            .map_err(Error::from)
    }

    pub fn set_org_member(&self, org_id: i64, user_id: i64, role: &str) -> Result<(), Error> {
        if !matches!(role, "owner" | "admin" | "member") {
            return Err(Error::Core(scion_core::Error::Invalid(
                "invalid org role".into(),
            )));
        }
        self.conn(|c| {
            c.execute(
                "INSERT INTO org_members(org_id, user_id, role) VALUES (?1,?2,?3)
                 ON CONFLICT(org_id, user_id) DO UPDATE SET role=excluded.role",
                params![org_id, user_id, role],
            )?;
            Ok(())
        })
    }

    pub fn remove_org_member(&self, org_id: i64, user_id: i64) -> Result<(), Error> {
        self.conn(|c| {
            c.execute(
                "DELETE FROM org_members WHERE org_id=?1 AND user_id=?2",
                params![org_id, user_id],
            )?;
            Ok(())
        })
    }

    pub fn create_token(
        &self,
        user_id: i64,
        name: &str,
        max_role: Role,
        repo_ids: Option<Vec<i64>>,
        expires_at: Option<i64>,
    ) -> Result<(i64, String), Error> {
        let (token, raw) = random_token();
        let hash = api_hash(&raw);
        let prefix = token.chars().take(8).collect::<String>();
        let repos = repo_ids.as_ref().map(|ids| {
            ids.iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",")
        });
        let id = self.conn(|c| {
            c.execute(
                "INSERT INTO api_tokens(user_id, name, token_hash, prefix, max_role, repo_ids, expires_at, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![user_id, name, hash.as_slice(), prefix, max_role.as_str(), repos, expires_at, now()],
            )?;
            Ok(c.last_insert_rowid())
        })?;
        Ok((id, token))
    }

    pub fn authenticate_token(&self, token: &str) -> Result<Option<Principal>, Error> {
        let raw = match hex::decode(token) {
            Ok(b) if b.len() == 32 => b,
            _ => return Ok(None),
        };
        let hash = api_hash(&raw);
        let row = self.conn(|c| {
            c.query_row(
                "SELECT t.id, t.user_id, t.max_role, t.repo_ids, t.expires_at, t.revoked_at, a.slug, u.is_site_admin, u.status
                 FROM api_tokens t JOIN users u ON u.id = t.user_id JOIN accounts a ON a.id = u.id
                 WHERE t.token_hash=?1",
                [hash.as_slice()],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()
        })?;
        let Some((tid, user_id, max_role, repos, exp, revoked, username, admin, status)) = row
        else {
            return Ok(None);
        };
        if revoked.is_some() || status != "active" || exp.unwrap_or(i64::MAX) < now() {
            return Ok(None);
        }
        let _ = self.conn(|c| {
            c.execute(
                "UPDATE api_tokens SET last_used_at=?1 WHERE id=?2",
                params![now(), tid],
            )?;
            Ok(())
        });
        let repos = repos.map(|s| {
            s.split(',')
                .filter_map(|p| p.parse().ok())
                .collect::<Vec<i64>>()
        });
        Ok(Some(Principal {
            user_id: Some(user_id),
            username,
            site_admin: admin != 0,
            via: Via::Token {
                max_role: Role::parse(&max_role).unwrap_or(Role::Read),
                repos,
            },
            session_hash: None,
            csrf: None,
        }))
    }

    pub fn list_tokens(&self, user_id: i64) -> Result<Vec<TokenInfo>, Error> {
        self.conn(|c| {
            let mut stmt = c.prepare(
                "SELECT id, name, prefix, max_role, expires_at FROM api_tokens
                 WHERE user_id=?1 AND revoked_at IS NULL ORDER BY id DESC",
            )?;
            let rows = stmt.query_map([user_id], |r| {
                Ok(TokenInfo {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    prefix: r.get(2)?,
                    max_role: r.get(3)?,
                    expires_at: r.get(4)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn revoke_token(&self, user_id: i64, token_id: i64) -> Result<(), Error> {
        self.conn(|c| {
            c.execute(
                "UPDATE api_tokens SET revoked_at=?1 WHERE id=?2 AND user_id=?3",
                params![now(), token_id, user_id],
            )?;
            Ok(())
        })
    }

    pub fn find_user_id(&self, username: &str) -> Result<Option<i64>, Error> {
        self.conn(|c| {
            c.query_row(
                "SELECT id FROM accounts WHERE slug=?1 COLLATE NOCASE AND kind='user'",
                [username],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn delete_repo(&self, repo_id: i64) -> Result<(), Error> {
        self.conn(|c| {
            c.execute(
                "UPDATE repos SET deleted_at=?1 WHERE id=?2",
                params![now(), repo_id],
            )?;
            Ok(())
        })
    }

    pub fn update_repo(
        &self,
        repo_id: i64,
        description: Option<&str>,
        default_ref: Option<&str>,
    ) -> Result<(), Error> {
        self.conn(|c| {
            if let Some(description) = description {
                c.execute(
                    "UPDATE repos SET description=?1 WHERE id=?2",
                    params![description, repo_id],
                )?;
            }
            if let Some(default_ref) = default_ref {
                c.execute(
                    "UPDATE repos SET default_ref=?1 WHERE id=?2",
                    params![default_ref, repo_id],
                )?;
            }
            Ok(())
        })
    }

    pub fn list_audit(
        &self,
        repo_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<(i64, String, String, String)>, Error> {
        self.conn(|c| {
            if let Some(repo_id) = repo_id {
                let mut stmt = c.prepare(
                    "SELECT ts, action, outcome, COALESCE(detail,'') FROM audit WHERE repo_id=?1 ORDER BY id DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![repo_id, limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
                rows.collect()
            } else {
                let mut stmt = c.prepare(
                    "SELECT ts, action, outcome, COALESCE(detail,'') FROM audit ORDER BY id DESC LIMIT ?1",
                )?;
                let rows = stmt.query_map([limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
                rows.collect()
            }
        })
    }

    pub fn must_change(&self, user_id: i64) -> Result<bool, Error> {
        self.conn(|c| {
            c.query_row(
                "SELECT must_change_password FROM users WHERE id=?1",
                [user_id],
                |r| r.get::<_, i64>(0),
            )
            .map(|v| v != 0)
        })
    }

    pub fn list_sessions(&self, user_id: i64) -> Result<Vec<(String, i64, i64)>, Error> {
        self.conn(|c| {
            let mut stmt = c.prepare(
                "SELECT id, created_at, last_seen_at FROM sessions
                 WHERE user_id=?1 AND revoked_at IS NULL ORDER BY last_seen_at DESC",
            )?;
            let rows = stmt.query_map([user_id], |r| {
                let id: Vec<u8> = r.get(0)?;
                Ok((hex::encode(id), r.get(1)?, r.get(2)?))
            })?;
            rows.collect()
        })
    }

    pub fn revoke_session(&self, user_id: i64, id_hex: &str) -> Result<(), Error> {
        let id = hex::decode(id_hex).map_err(|_| Error::NotFound)?;
        self.conn(|c| {
            c.execute(
                "UPDATE sessions SET revoked_at=?1 WHERE user_id=?2 AND id=?3 AND revoked_at IS NULL",
                params![now(), user_id, id],
            )?;
            Ok(())
        })
    }

    pub fn list_orgs(&self, user_id: i64) -> Result<Vec<(i64, String, String)>, Error> {
        self.conn(|c| {
            let mut stmt = c.prepare(
                "SELECT o.id, a.slug, m.role FROM org_members m
                 JOIN orgs o ON o.id = m.org_id
                 JOIN accounts a ON a.id = o.id
                 WHERE m.user_id=?1 ORDER BY a.slug",
            )?;
            let rows = stmt.query_map([user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
    }

    pub fn list_org_members(&self, org_id: i64) -> Result<Vec<(i64, String, String)>, Error> {
        self.conn(|c| {
            let mut stmt = c.prepare(
                "SELECT m.user_id, a.slug, m.role FROM org_members m
                 JOIN accounts a ON a.id = m.user_id WHERE m.org_id=?1 ORDER BY a.slug",
            )?;
            let rows = stmt.query_map([org_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
    }
}

fn glob_match(pattern: &str, path: &str) -> bool {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher().is_match(path))
        .unwrap_or(false)
}

pub fn namespace_for(repo_id: i64) -> String {
    format!("repo:{repo_id}")
}
