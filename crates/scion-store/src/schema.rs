pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS schema_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS changesets (
  ns TEXT NOT NULL,
  id BLOB NOT NULL,
  seq INTEGER NOT NULL,
  parents BLOB NOT NULL,
  body BLOB NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (ns, id)
) WITHOUT ROWID;
CREATE UNIQUE INDEX IF NOT EXISTS changesets_seq ON changesets(seq);

CREATE TABLE IF NOT EXISTS refs (
  ns TEXT NOT NULL,
  name TEXT NOT NULL,
  target BLOB NOT NULL,
  version INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (ns, name)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS ref_log (
  seq INTEGER PRIMARY KEY,
  ns TEXT NOT NULL,
  name TEXT NOT NULL,
  old BLOB,
  new BLOB NOT NULL,
  actor TEXT NOT NULL,
  ts INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS landings (
  ns TEXT NOT NULL,
  ref TEXT NOT NULL,
  changeset BLOB NOT NULL,
  landed_as BLOB NOT NULL,
  ref_seq INTEGER NOT NULL,
  ts INTEGER NOT NULL,
  actor TEXT NOT NULL,
  PRIMARY KEY (ns, ref, ref_seq)
);
CREATE INDEX IF NOT EXISTS landings_by_cs ON landings(ns, changeset);
CREATE INDEX IF NOT EXISTS landings_by_landed ON landings(ns, landed_as);

CREATE TABLE IF NOT EXISTS derived (
  kind TEXT NOT NULL,
  ver INTEGER NOT NULL,
  key BLOB NOT NULL,
  val BLOB NOT NULL,
  PRIMARY KEY (kind, ver, key)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS trees (
  id BLOB PRIMARY KEY,
  body BLOB NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS blob_index (
  id BLOB PRIMARY KEY,
  size INTEGER NOT NULL,
  stored_size INTEGER NOT NULL,
  layout INTEGER NOT NULL,
  flags INTEGER NOT NULL,
  created_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS blob_manifest (
  id BLOB NOT NULL,
  idx INTEGER NOT NULL,
  chunk BLOB NOT NULL,
  off INTEGER NOT NULL,
  len INTEGER NOT NULL,
  PRIMARY KEY (id, idx)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS idempotency (
  principal TEXT NOT NULL,
  key TEXT NOT NULL,
  response BLOB NOT NULL,
  expires_at INTEGER NOT NULL,
  PRIMARY KEY (principal, key)
);

CREATE TABLE IF NOT EXISTS path_history (
  ns TEXT NOT NULL,
  path TEXT NOT NULL,
  seq INTEGER NOT NULL,
  changeset BLOB NOT NULL,
  PRIMARY KEY (ns, path, seq)
);

CREATE TABLE IF NOT EXISTS object_domain (
  blob BLOB NOT NULL,
  ns TEXT NOT NULL,
  domain TEXT NOT NULL,
  PRIMARY KEY (blob, ns, domain)
);

CREATE TABLE IF NOT EXISTS git_map (
  ns TEXT NOT NULL,
  git_oid TEXT NOT NULL,
  changeset BLOB NOT NULL,
  PRIMARY KEY (ns, git_oid)
);

CREATE TABLE IF NOT EXISTS accounts (
  id INTEGER PRIMARY KEY,
  slug TEXT NOT NULL UNIQUE COLLATE NOCASE,
  kind TEXT NOT NULL CHECK (kind IN ('user','org')),
  display_name TEXT,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS users (
  id INTEGER PRIMARY KEY REFERENCES accounts(id),
  user_kind TEXT NOT NULL CHECK (user_kind IN ('human','agent','service')),
  password_hash TEXT,
  is_site_admin INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled')),
  must_change_password INTEGER NOT NULL DEFAULT 0,
  failed_logins INTEGER NOT NULL DEFAULT 0,
  locked_until INTEGER,
  last_login_at INTEGER,
  password_changed_at INTEGER
);

CREATE TABLE IF NOT EXISTS sessions (
  id BLOB PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id),
  csrf_secret BLOB NOT NULL,
  created_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  idle_expires_at INTEGER NOT NULL,
  absolute_expires_at INTEGER NOT NULL,
  ip TEXT,
  user_agent TEXT,
  revoked_at INTEGER
);
CREATE INDEX IF NOT EXISTS sessions_user ON sessions(user_id);

CREATE TABLE IF NOT EXISTS api_tokens (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id),
  name TEXT NOT NULL,
  token_hash BLOB NOT NULL UNIQUE,
  prefix TEXT NOT NULL,
  max_role TEXT NOT NULL,
  repo_ids TEXT,
  expires_at INTEGER,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER,
  revoked_at INTEGER
);

CREATE TABLE IF NOT EXISTS org_members (
  org_id INTEGER REFERENCES accounts(id),
  user_id INTEGER REFERENCES users(id),
  role TEXT NOT NULL CHECK (role IN ('owner','admin','member')),
  PRIMARY KEY (org_id, user_id)
);

CREATE TABLE IF NOT EXISTS orgs (
  id INTEGER PRIMARY KEY REFERENCES accounts(id),
  default_repo_role TEXT NOT NULL DEFAULT 'none' CHECK (default_repo_role IN ('none','read','write'))
);

CREATE TABLE IF NOT EXISTS repos (
  id INTEGER PRIMARY KEY,
  owner_id INTEGER NOT NULL REFERENCES accounts(id),
  name TEXT NOT NULL COLLATE NOCASE,
  description TEXT NOT NULL DEFAULT '',
  default_ref TEXT NOT NULL DEFAULT 'main',
  visibility TEXT NOT NULL DEFAULT 'private',
  created_at INTEGER NOT NULL,
  deleted_at INTEGER,
  UNIQUE (owner_id, name)
);

CREATE TABLE IF NOT EXISTS repo_grants (
  repo_id INTEGER REFERENCES repos(id),
  principal_type TEXT NOT NULL CHECK (principal_type IN ('user','team')),
  principal_id INTEGER NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('read','write','maintain','admin')),
  granted_by INTEGER,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (repo_id, principal_type, principal_id)
);

CREATE TABLE IF NOT EXISTS repo_path_rules (
  id INTEGER PRIMARY KEY,
  repo_id INTEGER NOT NULL REFERENCES repos(id),
  path_glob TEXT NOT NULL,
  principal_type TEXT,
  principal_id INTEGER,
  can_read INTEGER NOT NULL,
  can_write INTEGER NOT NULL,
  created_by INTEGER,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ref_protection (
  repo_id INTEGER NOT NULL,
  ref_pattern TEXT NOT NULL,
  min_role_to_land TEXT NOT NULL,
  require_review INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (repo_id, ref_pattern)
);

CREATE TABLE IF NOT EXISTS login_attempts (
  id INTEGER PRIMARY KEY,
  ts INTEGER NOT NULL,
  username_norm TEXT NOT NULL,
  source TEXT NOT NULL,
  success INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS login_attempts_user_ts ON login_attempts(username_norm, ts);

CREATE TABLE IF NOT EXISTS land_events (
  id INTEGER PRIMARY KEY,
  repo_id INTEGER,
  ref TEXT,
  changeset BLOB,
  outcome TEXT NOT NULL,
  attempts INTEGER NOT NULL,
  actor_user_id INTEGER,
  detail TEXT,
  ts INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS audit (
  id INTEGER PRIMARY KEY,
  ts INTEGER NOT NULL,
  actor_user_id INTEGER,
  actor_via TEXT,
  request_id TEXT,
  source TEXT,
  action TEXT NOT NULL,
  repo_id INTEGER,
  target TEXT,
  outcome TEXT NOT NULL CHECK (outcome IN ('allowed','denied','error')),
  detail TEXT
);
CREATE INDEX IF NOT EXISTS audit_repo_ts ON audit(repo_id, ts);
CREATE INDEX IF NOT EXISTS audit_actor_ts ON audit(actor_user_id, ts);

CREATE TABLE IF NOT EXISTS reviews (
  id INTEGER PRIMARY KEY,
  ns TEXT NOT NULL,
  changeset BLOB NOT NULL,
  author_user INTEGER,
  state TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS review_comments (
  id INTEGER PRIMARY KEY,
  review_id INTEGER NOT NULL,
  author_user INTEGER,
  body TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
"#;

pub const SMALL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS small_blobs (
  id BLOB PRIMARY KEY,
  data BLOB NOT NULL
) WITHOUT ROWID;
"#;
