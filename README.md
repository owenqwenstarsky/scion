# Scion

A local, server-centric source-control system. Clients stay online. The server owns commits, merges, history, and blame. This is a clean-room implementation of the architecture described in the attached plan, which itself was written from Anthropic's public description of its internal system. It is not that system and it is not affiliated with Anthropic.

The server process runs only in Docker. The `scion` binary on the host is a client. It starts and stops the stack; it does not open the database.

## Layout

| Path | Role |
| --- | --- |
| `crates/scion-core` | Canonical ids, paths, changesets, trees |
| `crates/scion-store` | SQLite metadata and content-addressed blobs |
| `crates/scion-merge` | Line-level rebase / diff3 |
| `crates/scion-derived` | Manifests, commit graph, blame, history, diffs |
| `crates/scion-iam` | Argon2id passwords, sessions, roles, path rules |
| `crates/scion-server` | Container-only binary and the JSON API |
| `crates/scion-client`, `crates/scion-cli` | Host client and `scion` command |
| `crates/scion-vfs` | In-memory virtual checkout (no FUSE/NFS mount) |
| `crates/scion-git-bridge` | Loose Git object import and export |
| `crates/scion-sim` | Concurrent compare-and-swap invariant check |
| `crates/scion-search`, `crates/scion-review` | Small helpers, not full products |
| `apps/web` | Next.js UI. A backend-for-frontend, not an authorizer |
| `docker/` | Server image, web image, Caddy, Compose |
| `openapi/scion.v1.json` | Hand-written subset of the JSON API |
| `proto/scion/v1/scion.proto` | gRPC shape. Not mounted |

`xtask check-layers` refuses a dependency from the client, CLI, VFS, or git bridge onto the store, derived data, IAM, or server.

## Run the stack

Requirements: Docker with Compose v2, and a built `scion` binary (`cargo build -p scion-cli`). From the repository root:

```bash
cargo build -p scion-cli
./target/debug/scion server up
```

`scion server up`:

1. Writes `docker/secrets/` (mode `0700` on the directory): bootstrap token, web service token, a self-signed certificate for `localhost` and `scion-server`, and the CA.
2. Builds and starts Compose project `scion`. The server listens on `0.0.0.0:8443`. Caddy publishes the UI on `0.0.0.0:8444`.
3. Creates the first administrator once. The username is `admin`. The password is printed once and saved to `docker/secrets/initial_admin_password` (mode `0400`).

Override ports with `SCION_SERVER_PORT` and `SCION_WEB_PORT`.

Other lifecycle commands: `scion server down`, `logs`, `status`, `restart`, `backup <dir>`. Restore is deliberately not a one-shot: stop the stack and replace the `scion-data` volume from a backup, then start again.

The server binary exits **78** unless `/.dockerenv` exists or `SCION_IN_CONTAINER=1`. It also exits unless a bootstrap token (at least 32 bytes), an existing user database, or a client CA is configured. Plaintext needs both `SCION_TLS_MODE=disabled` and `SCION_ALLOW_INSECURE_PLAINTEXT=1`. The published listen address must be `0.0.0.0` (or `::`) unless `SCION_ALLOW_LOOPBACK=1`.

Open the UI at `https://localhost:8444`. The certificate is the one just generated; trust `docker/secrets/ca.crt` or accept the warning. Sign in as `admin`, then change the password. The temporary password cannot do anything else.

`scion server down` stops the containers. The named volume `scion-data` keeps the database.

The server and web containers drop every Linux capability. Caddy does not: its binary has file capabilities, and `cap_drop: ALL` makes `exec` fail. Caddy still runs read-only, with `no-new-privileges`, and with writable mounts only on tmpfs. An init container (root, one shot) `chown`s the data volume to uid 65532 before the server starts; the server process itself is not root.

On some hosts Docker's bridge is filtered by iptables-legacy while the daemon programs nftables. If the UI container cannot reach the web container (Caddy returns 502, and a request from one container to another times out) while the host can still reach a container IP, allow forwarding on the Compose bridge. Discover it with `docker network inspect scion_backend` and accept that interface on the `DOCKER-FORWARD` chain. That rule is host state, not part of this repository, and it has to be recreated if the network is.

## Using the host CLI

Host state lives in `~/.scion-vcs/` (`SCION_HOME` overrides that).

```bash
scion auth login --server https://127.0.0.1:8443 --insecure --username admin --password "$(cat docker/secrets/initial_admin_password)"
scion repo create widgets --description demo
scion clone admin/widgets
# edit files
scion commit -m "initial"
scion land
scion log
scion blame README
scion update
scion sparse set 'include src/**'
```

`scion commit` uploads a changeset. `scion land` publishes it by compare-and-swap on one ref, rebasing if the ref moved. Sparse checkout is a client filter: the server still authorizes every path the account can see, and `scion update` only writes the paths the sparse spec matches.

## Accounts and permissions

Passwords are argon2id (19 MiB, t=2, p=1). Tests use a much smaller cost. Sessions last 12 hours idle and 7 days absolute, in an httpOnly cookie when you use the web UI. Login failures lock the account after 5 tries. Unknown usernames are verified against a dummy hash and return the same error as a bad password.

Roles, low to high: `read`, `write`, `maintain`, `admin`, plus `owner` for the repository owner. Grants cannot assign `owner`. API tokens only narrow the caller's role. A repository you cannot read is a 404, not a 403. Path rules only deny. A site admin can manage users and the global audit log, and does not automatically read repository contents.

The Next.js app stores the session and CSRF cookies (`__Host-scion_session`, `__Host-scion_csrf`, both httpOnly, Secure, SameSite=Lax) and forwards them with the web service token. It does not decide who can see a repository.

## Develop

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p xtask -- check-layers
```

Web:

```bash
cd apps/web
pnpm install
pnpm lint
pnpm typecheck
pnpm build
```

For a local UI against a plaintext server (only for development):

```bash
export SCION_IN_CONTAINER=1 SCION_TLS_MODE=disabled SCION_ALLOW_INSECURE_PLAINTEXT=1
export SCION_ALLOW_LOOPBACK=1 SCION_LISTEN=127.0.0.1:8080 SCION_ARGON_FAST=1
export SCION_BOOTSTRAP_TOKEN_FILE=... SCION_WEB_TOKEN_FILE=... SCION_DATA=/tmp/scion
./target/debug/scion-server serve
```

Then in `apps/web`: `SCION_SERVER_URL=http://127.0.0.1:8080 SCION_WEB_TOKEN=... pnpm dev`.

Playwright (`apps/web/e2e`) covers login, forced password change, and the fact that a second user cannot see a repository until they are granted read. Point it at a running UI with `SCION_WEB_ORIGIN`.

## What is not done

These are real gaps, not a backlog disguised as a feature list.

- gRPC is specified in `proto/scion/v1/scion.proto` and is not served. Clients speak JSON.
- No FUSE or NFS mount. `scion-vfs` is an in-memory overlay with a model test. The server never mounts a filesystem.
- Git bridge reads and writes loose objects only. Packfiles, alternates, and rename detection are absent.
- No multi-node replication, no Windows support, no TOTP, no public repositories.
- Code search is a substring scan of paths and text the caller already has. Reviews are an in-memory decision (an author cannot self-approve) with no stored comments and no UI.
- Ref delete is audited and not applied.
- Changeset bodies are inline zstd, not spilled to blobs.
- The writer is a process mutex, not a group-commit actor. There is no 1M-changeset benchmark.
- Garbage collection is best-effort and lightly tested.
- Blame walks first parents. It is not rename-aware.
- Sparse checkout does not ask the server for a cone. The client filters a full tree listing.
- OpenAPI is a hand-written subset.
- The phase-0 spikes in the plan (macFUSE, NFS on macOS, a redb bake-off, Docker Desktop fsync, multi-arch images) were not measured. See `docs/adr/`.

Design notes that differ from a literal reading of the plan are in `docs/adr/`.
