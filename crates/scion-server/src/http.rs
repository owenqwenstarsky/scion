//! JSON façade. This is the wire protocol the CLI and the web BFF speak.
//! Authorization is decided here, in-process, by `scion-iam`.

use crate::app::{token_eq, App};
use crate::error::ApiError;
use crate::land::{self, LandRequest, Strategy};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use scion_core::{BlobId, Change, Changeset, ChangesetId, FileKind, RepoPath};
use scion_iam::{capabilities, namespace_for, Action, AuthzError, Principal, Role, Via};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct ApiState {
    pub app: Arc<App>,
}

pub fn router(app: Arc<App>) -> Router {
    let state = ApiState { app };
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/stats", get(stats))
        .route("/v1/setup/first-admin", post(first_admin))
        .route("/v1/auth/login", post(login))
        .route("/v1/auth/logout", post(logout))
        .route("/v1/auth/change-password", post(change_password))
        .route("/v1/auth/session", get(session_info))
        .route("/v1/auth/sessions", get(list_sessions))
        .route("/v1/auth/sessions/{id}", delete(revoke_session))
        .route("/v1/users", get(list_users).post(create_user))
        .route("/v1/users/{id}/status", post(set_user_status))
        .route("/v1/users/{id}/unlock", post(unlock_user))
        .route("/v1/users/{id}/reset-password", post(reset_password))
        .route("/v1/users/{id}/site-admin", post(set_site_admin))
        .route("/v1/orgs", get(list_orgs).post(create_org))
        .route(
            "/v1/orgs/{id}/members",
            get(list_org_members).post(set_org_member),
        )
        .route("/v1/orgs/{id}/members/{user}", delete(remove_org_member))
        .route("/v1/repos", get(list_repos).post(create_repo))
        .route(
            "/v1/repos/{owner}/{name}",
            get(get_repo).patch(patch_repo).delete(delete_repo),
        )
        .route(
            "/v1/repos/{owner}/{name}/capabilities",
            get(repo_capabilities),
        )
        .route(
            "/v1/repos/{owner}/{name}/grants",
            get(list_grants).put(set_grant),
        )
        .route(
            "/v1/repos/{owner}/{name}/grants/{user}",
            delete(remove_grant),
        )
        .route(
            "/v1/repos/{owner}/{name}/path-rules",
            get(list_paths).post(add_path),
        )
        .route("/v1/repos/{owner}/{name}/audit", get(repo_audit))
        .route("/v1/audit", get(global_audit))
        .route("/v1/tokens", get(list_tokens).post(create_token))
        .route("/v1/tokens/{id}", delete(revoke_token))
        .route("/v1/repos/{owner}/{name}/blobs", put(put_blob))
        .route("/v1/repos/{owner}/{name}/blobs/{id}", get(get_blob))
        .route(
            "/v1/repos/{owner}/{name}/changesets",
            post(create_changeset),
        )
        .route(
            "/v1/repos/{owner}/{name}/changesets/{id}",
            get(get_changeset),
        )
        .route("/v1/repos/{owner}/{name}/land", post(land_changeset))
        .route("/v1/repos/{owner}/{name}/edits", post(apply_edits))
        .route(
            "/v1/repos/{owner}/{name}/refs",
            get(list_refs).put(put_ref).delete(delete_ref),
        )
        .route("/v1/repos/{owner}/{name}/tree", get(tree))
        .route("/v1/repos/{owner}/{name}/file", get(file))
        .route("/v1/repos/{owner}/{name}/log", get(log))
        .route("/v1/repos/{owner}/{name}/blame", get(blame))
        .route("/v1/repos/{owner}/{name}/diff", get(diff))
        .route("/v1/repos/{owner}/{name}/history", get(history))
        .route("/v1/repos/{owner}/{name}/landings", get(landings))
        .route("/v1/repos/{owner}/{name}/merge-base", get(merge_base))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

struct Access {
    mutating: bool,
    password_flow: bool,
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("authorization")?.to_str().ok()?;
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
}

enum Caller {
    Bootstrap,
    Web,
    User(Principal),
}

fn identify(app: &App, headers: &HeaderMap) -> Result<Caller, ApiError> {
    if let Some(token) = bearer(headers) {
        if let Some(hash) = &app.bootstrap_hash {
            if token_eq(hash, token, "scion bootstrap v1") {
                return Ok(Caller::Bootstrap);
            }
        }
        if let Some(hash) = &app.web_hash {
            if token_eq(hash, token, "scion web service v1") {
                return session_or_web(app, headers);
            }
        }
        if let Some(principal) = app.accounts.authenticate_token(token)? {
            return Ok(Caller::User(principal));
        }
        return Err(ApiError::unauthorized());
    }
    if let Some(session) = header(headers, "x-scion-session") {
        if let Some(principal) = app.accounts.authenticate_session(session)? {
            return Ok(Caller::User(principal));
        }
    }
    Err(ApiError::unauthorized())
}

fn session_or_web(app: &App, headers: &HeaderMap) -> Result<Caller, ApiError> {
    let Some(session) = header(headers, "x-scion-session") else {
        return Ok(Caller::Web);
    };
    match app.accounts.authenticate_session(session)? {
        Some(principal) => Ok(Caller::User(principal)),
        None => Err(ApiError::unauthorized()),
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn user(app: &App, headers: &HeaderMap, access: Access) -> Result<Principal, ApiError> {
    let Caller::User(principal) = identify(app, headers)? else {
        return Err(ApiError::unauthorized());
    };
    if access.mutating && matches!(principal.via, Via::Session) {
        let presented = header(headers, "x-scion-csrf").unwrap_or("");
        if !scion_iam::Accounts::csrf_matches(&principal, presented) {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "csrf",
                "csrf token missing or invalid",
            ));
        }
    }
    if !access.password_flow {
        if let Some(id) = principal.user_id {
            if app.accounts.must_change(id)? {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "must_change_password",
                    "password change required",
                ));
            }
        }
    }
    Ok(principal)
}

fn read_user(app: &App, headers: &HeaderMap) -> Result<Principal, ApiError> {
    user(
        app,
        headers,
        Access {
            mutating: false,
            password_flow: false,
        },
    )
}

fn write_user(app: &App, headers: &HeaderMap) -> Result<Principal, ApiError> {
    user(
        app,
        headers,
        Access {
            mutating: true,
            password_flow: false,
        },
    )
}

fn site_admin(principal: &Principal) -> Result<(), ApiError> {
    if principal.site_admin {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

struct RepoRef {
    id: i64,
    default_ref: String,
    ns: String,
}

fn open_repo(app: &App, owner: &str, name: &str) -> Result<RepoRef, ApiError> {
    let Some((id, _owner, default_ref)) = app.accounts.repo_by_name(owner, name)? else {
        return Err(ApiError::not_found());
    };
    Ok(RepoRef {
        id,
        default_ref,
        ns: namespace_for(id),
    })
}

fn auth_repo(
    app: &App,
    principal: &Principal,
    repo: &RepoRef,
    action: Action,
    path: Option<&str>,
) -> Result<(), ApiError> {
    app.accounts
        .authorize(principal, repo.id, action, path)
        .map_err(authz_error)
}

fn authz_error(err: AuthzError) -> ApiError {
    match err {
        AuthzError::NotFound => ApiError::not_found(),
        AuthzError::Forbidden => ApiError::forbidden(),
        AuthzError::Unauthenticated => ApiError::unauthorized(),
    }
}

fn via_name(principal: &Principal) -> &'static str {
    match principal.via {
        Via::Session => "session",
        Via::Token { .. } => "token",
        Via::Service => "service",
        Via::Bootstrap => "bootstrap",
    }
}

async fn block<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "join", err.to_string()))
}

fn idem_key(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(key) = header(headers, "idempotency-key") else {
        return Ok(None);
    };
    if key.is_empty() || key.len() > 200 {
        return Err(ApiError::bad("invalid idempotency key"));
    }
    Ok(Some(key.to_string()))
}

fn idem_load(app: &App, user_id: i64, key: &Option<String>) -> Result<Option<Value>, ApiError> {
    let Some(key) = key else {
        return Ok(None);
    };
    match app.store.idem_get(&format!("u{user_id}"), key)? {
        Some(bytes) => {
            let value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

fn idem_store(app: &App, user_id: i64, key: &Option<String>, body: &Value) {
    let Some(key) = key else {
        return;
    };
    if let Ok(bytes) = serde_json::to_vec(body) {
        let _ = app
            .store
            .idem_put(&format!("u{user_id}"), key, &bytes, 86_400);
    }
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn stats(State(state): State<ApiState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let app = state.app.clone();
    let principal = read_user(&app, &headers)?;
    if !principal.site_admin {
        return Err(ApiError::forbidden());
    }
    let report = app.store.doctor()?;
    Ok(Json(json!({
        "version": app.version,
        "plaintext": app.plaintext,
        "uptime_secs": app.started.elapsed().as_secs(),
        "changesets": report.changesets,
        "refs": report.refs,
        "blobs": report.blobs,
        "problems": report.problems,
    })))
}

#[derive(Deserialize)]
struct PasswordBody {
    username: String,
    password: String,
}

async fn first_admin(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<PasswordBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let app = state.app.clone();
    match identify(&app, &headers)? {
        Caller::Bootstrap => {}
        _ => return Err(ApiError::unauthorized()),
    }
    let username = body.username.clone();
    let password = body.password.clone();
    let id = block(move || app.accounts.create_first_admin(&username, &password)).await??;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": id, "username": body.username})),
    ))
}

async fn login(
    State(state): State<ApiState>,
    Json(body): Json<PasswordBody>,
) -> Result<Json<Value>, ApiError> {
    let app = state.app.clone();
    let username = body.username.clone();
    let password = body.password.clone();
    let result = block(move || app.accounts.login(&username, &password, "api")).await??;
    Ok(Json(json!({
        "session_token": result.session_token,
        "csrf_token": result.csrf_token,
        "must_change_password": result.must_change_password,
        "user_id": result.user_id,
        "username": result.username,
        "site_admin": result.site_admin,
    })))
}

async fn logout(State(state): State<ApiState>, headers: HeaderMap) -> Result<StatusCode, ApiError> {
    let app = state.app.clone();
    let _principal = user(
        &app,
        &headers,
        Access {
            mutating: true,
            password_flow: true,
        },
    )?;
    if let Some(session) = header(&headers, "x-scion-session") {
        app.accounts.logout(session)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ChangePasswordBody {
    current: String,
    new_password: String,
}

async fn change_password(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordBody>,
) -> Result<StatusCode, ApiError> {
    let app = state.app.clone();
    let principal = user(
        &app,
        &headers,
        Access {
            mutating: true,
            password_flow: true,
        },
    )?;
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    let current = body.current.clone();
    let next = body.new_password.clone();
    block(move || app.accounts.change_password(user_id, &current, &next)).await??;
    Ok(StatusCode::NO_CONTENT)
}

async fn session_info(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let app = state.app.clone();
    let principal = user(
        &app,
        &headers,
        Access {
            mutating: false,
            password_flow: true,
        },
    )?;
    let must = principal
        .user_id
        .map(|id| app.accounts.must_change(id))
        .transpose()?
        .unwrap_or(false);
    Ok(Json(json!({
        "user_id": principal.user_id,
        "username": principal.username,
        "site_admin": principal.site_admin,
        "must_change_password": must,
        "via": via_name(&principal),
    })))
}

async fn list_sessions(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    let rows = state.app.accounts.list_sessions(user_id)?;
    let current = principal.session_hash.map(hex::encode);
    let sessions = rows
        .into_iter()
        .map(|(id, created, seen)| {
            json!({
                "id": id,
                "created_at": created,
                "last_seen_at": seen,
                "current": current.as_deref() == Some(id.as_str()),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({"sessions": sessions})))
}

async fn revoke_session(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    state.app.accounts.revoke_session(user_id, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_users(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    site_admin(&principal)?;
    let users = state.app.accounts.list_users()?;
    let body = users
        .into_iter()
        .map(|user| {
            json!({
                "id": user.id,
                "username": user.username,
                "kind": user.user_kind,
                "site_admin": user.site_admin,
                "status": user.status,
                "must_change_password": user.must_change_password,
                "locked_until": user.locked_until,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({"users": body})))
}

#[derive(Deserialize)]
struct CreateUserBody {
    username: String,
    password: Option<String>,
    kind: Option<String>,
    site_admin: Option<bool>,
}

async fn create_user(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateUserBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let app = state.app.clone();
    let principal = write_user(&app, &headers)?;
    site_admin(&principal)?;
    let username = body.username.clone();
    let password = body.password.clone();
    let kind = body.kind.clone().unwrap_or_else(|| "human".into());
    let admin = body.site_admin.unwrap_or(false);
    let (id, generated) = block(move || {
        app.accounts
            .create_user(&username, password.as_deref(), &kind, admin)
    })
    .await??;
    let _ = state.app.store.audit(
        principal.user_id,
        via_name(&principal),
        "user.create",
        None,
        &body.username,
        "allowed",
        "",
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": id, "username": body.username, "password": generated})),
    ))
}

#[derive(Deserialize)]
struct StatusBody {
    status: String,
}

async fn set_user_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<StatusBody>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    site_admin(&principal)?;
    if !matches!(body.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad("status must be active or disabled"));
    }
    state.app.accounts.set_status(id, &body.status)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unlock_user(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    site_admin(&principal)?;
    state.app.accounts.unlock(id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reset_password(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let app = state.app.clone();
    let principal = write_user(&app, &headers)?;
    site_admin(&principal)?;
    let password = block(move || app.accounts.reset_password(id)).await??;
    Ok(Json(json!({"password": password})))
}

#[derive(Deserialize)]
struct AdminFlag {
    admin: bool,
}

async fn set_site_admin(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<AdminFlag>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    site_admin(&principal)?;
    state.app.accounts.set_site_admin(id, body.admin)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_orgs(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    let orgs = state.app.accounts.list_orgs(user_id)?;
    Ok(Json(json!({
        "orgs": orgs.into_iter().map(|(id, slug, role)| json!({"id": id, "slug": slug, "role": role})).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
struct CreateOrgBody {
    slug: String,
    default_role: Option<String>,
}

async fn create_org(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateOrgBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    let id = state.app.accounts.create_org(
        user_id,
        &body.slug,
        body.default_role.as_deref().unwrap_or("none"),
    )?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": id, "slug": body.slug})),
    ))
}

async fn list_org_members(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    ensure_org_member(&state.app, &principal, id)?;
    let members = state.app.accounts.list_org_members(id)?;
    Ok(Json(json!({
        "members": members.into_iter().map(|(uid, name, role)| json!({"id": uid, "username": name, "role": role})).collect::<Vec<_>>()
    })))
}

fn ensure_org_member(app: &App, principal: &Principal, org_id: i64) -> Result<(), ApiError> {
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    let orgs = app.accounts.list_orgs(user_id)?;
    if orgs.iter().any(|(id, _, _)| *id == org_id) || principal.site_admin {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

#[derive(Deserialize)]
struct OrgMemberBody {
    username: String,
    role: String,
}

async fn set_org_member(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<OrgMemberBody>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    ensure_org_admin(&state.app, &principal, id)?;
    let Some(user_id) = state.app.accounts.find_user_id(&body.username)? else {
        return Err(ApiError::not_found());
    };
    state.app.accounts.set_org_member(id, user_id, &body.role)?;
    Ok(StatusCode::NO_CONTENT)
}

fn ensure_org_admin(app: &App, principal: &Principal, org_id: i64) -> Result<(), ApiError> {
    if principal.site_admin {
        return Ok(());
    }
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    let orgs = app.accounts.list_orgs(user_id)?;
    match orgs.iter().find(|(id, _, _)| *id == org_id) {
        Some((_, _, role)) if role == "owner" || role == "admin" => Ok(()),
        Some(_) => Err(ApiError::forbidden()),
        None => Err(ApiError::not_found()),
    }
}

async fn remove_org_member(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((id, user)): Path<(i64, String)>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    ensure_org_admin(&state.app, &principal, id)?;
    let Some(user_id) = state.app.accounts.find_user_id(&user)? else {
        return Err(ApiError::not_found());
    };
    state.app.accounts.remove_org_member(id, user_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_repos(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    let repos = state.app.accounts.list_repos(user_id)?;
    Ok(Json(json!({
        "repos": repos.into_iter().map(|repo| json!({
            "id": repo.id,
            "owner": repo.owner,
            "name": repo.name,
            "description": repo.description,
            "default_ref": repo.default_ref,
            "role": repo.role.as_str(),
        })).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
struct CreateRepoBody {
    name: String,
    description: Option<String>,
}

async fn create_repo(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<CreateRepoBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    let id = state.app.accounts.create_repo(
        user_id,
        &body.name,
        body.description.as_deref().unwrap_or(""),
    )?;
    let _ = state.app.store.audit(
        principal.user_id,
        via_name(&principal),
        "repo.create",
        Some(id),
        &body.name,
        "allowed",
        "",
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": id, "owner": principal.username, "name": body.name})),
    ))
}

async fn get_repo(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::ObjectRead, None)?;
    let role = state.app.accounts.role_for(&principal, repo.id)?;
    Ok(Json(json!({
        "id": repo.id,
        "owner": owner,
        "name": name,
        "default_ref": repo.default_ref,
        "role": role.as_str(),
        "capabilities": caps_json(role),
    })))
}

fn caps_json(role: Role) -> Value {
    let caps = capabilities(role);
    json!({
        "role": caps.role,
        "can_read": caps.can_read,
        "can_write": caps.can_write,
        "can_land": caps.can_land,
        "can_manage_refs": caps.can_manage_refs,
        "can_manage_members": caps.can_manage_members,
        "can_admin_repo": caps.can_admin_repo,
        "can_delete": caps.can_delete,
    })
}

#[derive(Deserialize)]
struct PatchRepoBody {
    description: Option<String>,
    default_ref: Option<String>,
}

async fn patch_repo(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Json(body): Json<PatchRepoBody>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::SettingsWrite, None)?;
    state.app.accounts.update_repo(
        repo.id,
        body.description.as_deref(),
        body.default_ref.as_deref(),
    )?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_repo(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::RepoDelete, None)?;
    state.app.accounts.delete_repo(repo.id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn repo_capabilities(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::ObjectRead, None)?;
    let role = state.app.accounts.role_for(&principal, repo.id)?;
    Ok(Json(caps_json(role)))
}

async fn list_grants(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::MembersManage, None)?;
    let grants = state.app.accounts.list_grants(repo.id)?;
    Ok(Json(json!({
        "grants": grants.into_iter().map(|(id, user, role)| json!({"id": id, "username": user, "role": role})).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
struct GrantBody {
    username: String,
    role: String,
}

async fn set_grant(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Json(body): Json<GrantBody>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::MembersManage, None)?;
    let Some(role) = Role::parse(&body.role) else {
        return Err(ApiError::bad("unknown role"));
    };
    let Some(user_id) = state.app.accounts.find_user_id(&body.username)? else {
        return Err(ApiError::not_found());
    };
    let Some(actor) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    state
        .app
        .accounts
        .set_grant(repo.id, user_id, role, actor)?;
    let _ = state.app.store.audit(
        principal.user_id,
        via_name(&principal),
        "grant.set",
        Some(repo.id),
        &body.username,
        "allowed",
        &body.role,
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_grant(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name, user)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::MembersManage, None)?;
    let Some(user_id) = state.app.accounts.find_user_id(&user)? else {
        return Err(ApiError::not_found());
    };
    state.app.accounts.remove_grant(repo.id, user_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_paths(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::PathRulesManage, None)?;
    let rules = state.app.accounts.list_path_rules(repo.id)?;
    Ok(Json(json!({
        "rules": rules.into_iter().map(|(id, glob, read, write)| json!({
            "id": id, "glob": glob, "can_read": read, "can_write": write
        })).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
struct PathRuleBody {
    glob: String,
    can_read: bool,
    can_write: bool,
    username: Option<String>,
}

async fn add_path(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Json(body): Json<PathRuleBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::PathRulesManage, None)?;
    let principal_id = match &body.username {
        Some(name) => Some(
            state
                .app
                .accounts
                .find_user_id(name)?
                .ok_or_else(ApiError::not_found)?,
        ),
        None => None,
    };
    let Some(actor) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    let id = state.app.accounts.set_path_rule(
        repo.id,
        &body.glob,
        principal_id,
        body.can_read,
        body.can_write,
        actor,
    )?;
    Ok((StatusCode::CREATED, Json(json!({"id": id}))))
}

async fn repo_audit(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::Query, None)?;
    Ok(Json(audit_json(&state.app, Some(repo.id))?))
}

async fn global_audit(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    site_admin(&principal)?;
    Ok(Json(audit_json(&state.app, None)?))
}

fn audit_json(app: &App, repo: Option<i64>) -> Result<Value, ApiError> {
    let rows = app.accounts.list_audit(repo, 200)?;
    Ok(json!({
        "events": rows.into_iter().map(|(ts, action, outcome, detail)| json!({
            "ts": ts, "action": action, "outcome": outcome, "detail": detail
        })).collect::<Vec<_>>()
    }))
}

async fn list_tokens(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    let tokens = state.app.accounts.list_tokens(user_id)?;
    Ok(Json(json!({
        "tokens": tokens.into_iter().map(|token| json!({
            "id": token.id, "name": token.name, "prefix": token.prefix, "max_role": token.max_role, "expires_at": token.expires_at
        })).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
struct TokenBody {
    name: String,
    max_role: String,
    repo_ids: Option<Vec<i64>>,
    expires_at: Option<i64>,
}

async fn create_token(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<TokenBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    let Some(role) = Role::parse(&body.max_role) else {
        return Err(ApiError::bad("unknown role"));
    };
    if role == Role::Owner || role == Role::None {
        return Err(ApiError::bad(
            "token role must be read, write, maintain, or admin",
        ));
    }
    let (id, token) = state.app.accounts.create_token(
        user_id,
        &body.name,
        role,
        body.repo_ids,
        body.expires_at,
    )?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": id, "token": token, "name": body.name})),
    ))
}

async fn revoke_token(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let Some(user_id) = principal.user_id else {
        return Err(ApiError::unauthorized());
    };
    state.app.accounts.revoke_token(user_id, id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn put_blob(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::ChangesetCreate, None)?;
    if body.len() > 64 * 1024 * 1024 {
        return Err(ApiError::bad("blob exceeds 64 MiB"));
    }
    let id = BlobId::of(&body);
    state.app.store.put_blob(id, &body)?;
    let _ = state
        .app
        .store
        .label_domain(id.as_bytes(), &repo.ns, "blob");
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": id.to_hex(), "size": body.len()})),
    ))
}

async fn get_blob(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name, id)): Path<(String, String, String)>,
) -> Result<axum::response::Response, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::ObjectRead, None)?;
    let id = BlobId::parse_hex(&id).map_err(|err| ApiError::bad(err.to_string()))?;
    let bytes = state.app.store.get_blob(id)?;
    Ok((StatusCode::OK, bytes).into_response())
}

#[derive(Deserialize)]
struct ChangeIn {
    path: String,
    op: String,
    kind: Option<String>,
    text: Option<String>,
    blob: Option<String>,
}

#[derive(Deserialize)]
struct ChangesetIn {
    parents: Vec<String>,
    message: String,
    changes: Vec<ChangeIn>,
}

fn materialize(app: &App, items: &[ChangeIn]) -> Result<BTreeMap<RepoPath, Change>, ApiError> {
    let mut changes = BTreeMap::new();
    for item in items {
        let path = RepoPath::new(&item.path).map_err(|err| ApiError::bad(err.to_string()))?;
        let change = match item.op.as_str() {
            "delete" => Change::Delete,
            "put" => put_change(app, item)?,
            _ => return Err(ApiError::bad("op must be put or delete")),
        };
        changes.insert(path, change);
    }
    Ok(changes)
}

fn put_change(app: &App, item: &ChangeIn) -> Result<Change, ApiError> {
    let kind = match item.kind.as_deref().unwrap_or("regular") {
        "regular" => FileKind::Regular,
        "executable" => FileKind::Executable,
        "symlink" => FileKind::Symlink,
        _ => return Err(ApiError::bad("bad file kind")),
    };
    if let Some(text) = &item.text {
        let bytes = text.as_bytes();
        let id = BlobId::of(bytes);
        app.store.put_blob(id, bytes)?;
        return Ok(Change::Put {
            kind,
            blob: id,
            size: bytes.len() as u64,
        });
    }
    let Some(blob) = &item.blob else {
        return Err(ApiError::bad("put requires text or blob"));
    };
    let id = BlobId::parse_hex(blob).map_err(|err| ApiError::bad(err.to_string()))?;
    let bytes = app.store.get_blob(id)?;
    Ok(Change::Put {
        kind,
        blob: id,
        size: bytes.len() as u64,
    })
}

fn changeset_view(cs: &Changeset) -> Value {
    json!({
        "id": cs.id.to_hex(),
        "parents": cs.parents.iter().map(|p| p.to_hex()).collect::<Vec<_>>(),
        "message": cs.message,
        "author": {"name": cs.author.name, "email": cs.author.email, "when": cs.author.when},
        "changes": cs.changes.iter().map(|(path, change)| change_view(path.as_str(), change)).collect::<Vec<_>>(),
    })
}

fn change_view(path: &str, change: &Change) -> Value {
    match change {
        Change::Delete => json!({"path": path, "op": "delete"}),
        Change::Put { kind, blob, size } => json!({
            "path": path,
            "op": "put",
            "kind": match kind {
                FileKind::Regular => "regular",
                FileKind::Executable => "executable",
                FileKind::Symlink => "symlink",
            },
            "blob": blob.to_hex(),
            "size": size,
        }),
    }
}

async fn create_changeset(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Json(body): Json<ChangesetIn>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let app = Arc::clone(&state.app);
    let principal = write_user(&app, &headers)?;
    let repo = open_repo(&app, &owner, &name)?;
    auth_repo(&app, &principal, &repo, Action::ChangesetCreate, None)?;
    for item in &body.changes {
        auth_repo(
            &app,
            &principal,
            &repo,
            Action::ChangesetCreate,
            Some(&item.path),
        )?;
    }
    let key = idem_key(&headers)?;
    if let Some(hit) = idem_load(&app, principal.user_id.unwrap_or(0), &key)? {
        return Ok((StatusCode::OK, Json(hit)));
    }
    let parents = body
        .parents
        .iter()
        .map(|p| ChangesetId::parse_hex(p).map_err(|err| ApiError::bad(err.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    let changes = materialize(&app, &body.changes)?;
    let identity = land::identity_for(&principal.username);
    let cs = Changeset::create(
        parents,
        changes,
        identity.clone(),
        identity,
        body.message,
        BTreeMap::new(),
    );
    app.store.put_changeset(&repo.ns, &cs)?;
    let view = changeset_view(&cs);
    idem_store(&app, principal.user_id.unwrap_or(0), &key, &view);
    Ok((StatusCode::CREATED, Json(view)))
}

async fn get_changeset(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name, id)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::Query, None)?;
    let id = ChangesetId::parse_hex(&id).map_err(|err| ApiError::bad(err.to_string()))?;
    let Some(cs) = state.app.store.get_changeset(&repo.ns, id)? else {
        return Err(ApiError::not_found());
    };
    Ok(Json(changeset_view(&cs)))
}

#[derive(Deserialize)]
struct LandBody {
    changeset: String,
    #[serde(rename = "ref")]
    ref_name: String,
    strategy: Option<String>,
}

async fn land_changeset(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Json(body): Json<LandBody>,
) -> Result<Json<Value>, ApiError> {
    let app = Arc::clone(&state.app);
    let principal = write_user(&app, &headers)?;
    let repo = open_repo(&app, &owner, &name)?;
    auth_repo(&app, &principal, &repo, Action::Land, None)?;
    let key = idem_key(&headers)?;
    let user_id = principal.user_id.unwrap_or(0);
    if let Some(hit) = idem_load(&app, user_id, &key)? {
        return Ok(Json(hit));
    }
    let changeset =
        ChangesetId::parse_hex(&body.changeset).map_err(|err| ApiError::bad(err.to_string()))?;
    let strategy = Strategy::parse(body.strategy.as_deref().unwrap_or("rebase"));
    let actor = principal.username.clone();
    let actor_id = principal.user_id;
    let ns = repo.ns.clone();
    let ref_name = body.ref_name.clone();
    let repo_id = repo.id;
    let outcome = block(move || {
        land::land(
            &app,
            &LandRequest {
                ns,
                repo_id,
                ref_name,
                changeset,
                strategy,
                actor,
                actor_id,
                max_attempts: 8,
            },
        )
    })
    .await?
    .map_err(land_error)?;
    let view = json!({
        "landed": outcome.landed.to_hex(),
        "rebased_from": outcome.rebased_from.map(|id| id.to_hex()),
        "attempts": outcome.attempts,
    });
    idem_store(&state.app, user_id, &key, &view);
    Ok(Json(view))
}

fn land_error(err: land::LandError) -> ApiError {
    match err {
        land::LandError::NotFound => ApiError::not_found(),
        land::LandError::NotFastForward => ApiError::new(
            StatusCode::CONFLICT,
            "not_fast_forward",
            "not a fast-forward",
        ),
        land::LandError::Conflict(conflicts) => {
            let mut api = ApiError::new(StatusCode::CONFLICT, "conflict", conflicts.to_string());
            api.extra = json!({
                "paths": conflicts.items.iter().map(|item| item.path.to_string()).collect::<Vec<_>>()
            });
            api
        }
        land::LandError::Contended { attempts } => ApiError::new(
            StatusCode::CONFLICT,
            "contended",
            format!("lost compare-and-swap after {attempts} attempts"),
        ),
        land::LandError::Store(err) => err.into(),
    }
}

#[derive(Deserialize)]
struct EditBody {
    #[serde(rename = "ref")]
    ref_name: Option<String>,
    message: String,
    strategy: Option<String>,
    changes: Vec<ChangeIn>,
}

async fn apply_edits(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Json(body): Json<EditBody>,
) -> Result<Json<Value>, ApiError> {
    let app = Arc::clone(&state.app);
    let principal = write_user(&app, &headers)?;
    let repo = open_repo(&app, &owner, &name)?;
    auth_repo(&app, &principal, &repo, Action::ApplyEdits, None)?;
    for item in &body.changes {
        auth_repo(
            &app,
            &principal,
            &repo,
            Action::ApplyEdits,
            Some(&item.path),
        )?;
    }
    let ref_name = body
        .ref_name
        .clone()
        .unwrap_or_else(|| repo.default_ref.clone());
    let parent = app
        .store
        .get_ref(&repo.ns, &ref_name)?
        .map(|tip| tip.target);
    let changes = materialize(&app, &body.changes)?;
    let identity = land::identity_for(&principal.username);
    let parents = parent.into_iter().collect::<Vec<_>>();
    let cs = Changeset::create(
        parents,
        changes,
        identity.clone(),
        identity,
        body.message,
        BTreeMap::new(),
    );
    app.store.put_changeset(&repo.ns, &cs)?;
    let strategy = Strategy::parse(body.strategy.as_deref().unwrap_or("rebase"));
    let actor = principal.username.clone();
    let actor_id = principal.user_id;
    let ns = repo.ns.clone();
    let repo_id = repo.id;
    let changeset = cs.id;
    let outcome = block(move || {
        land::land(
            &app,
            &LandRequest {
                ns,
                repo_id,
                ref_name,
                changeset,
                strategy,
                actor,
                actor_id,
                max_attempts: 8,
            },
        )
    })
    .await?
    .map_err(land_error)?;
    Ok(Json(json!({
        "changeset": cs.id.to_hex(),
        "landed": outcome.landed.to_hex(),
        "attempts": outcome.attempts,
    })))
}

async fn list_refs(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::Query, None)?;
    let refs = state.app.store.list_refs(&repo.ns)?;
    Ok(Json(json!({
        "refs": refs.into_iter().map(|(name, state)| json!({
            "name": name,
            "target": state.target.to_hex(),
            "version": state.version,
        })).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
struct RefBody {
    name: String,
    target: String,
}

async fn put_ref(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Json(body): Json<RefBody>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::RefCreate, None)?;
    let target =
        ChangesetId::parse_hex(&body.target).map_err(|err| ApiError::bad(err.to_string()))?;
    if state.app.store.get_changeset(&repo.ns, target)?.is_none() {
        return Err(ApiError::not_found());
    }
    let expected = state
        .app
        .store
        .get_ref(&repo.ns, &body.name)?
        .map(|r| r.target);
    let won = state.app.store.cas_ref(&scion_store::Cas {
        ns: repo.ns,
        name: body.name,
        expected,
        new_target: target,
        source: target,
        actor: principal.username,
    })?;
    if won {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            StatusCode::CONFLICT,
            "contended",
            "ref changed",
        ))
    }
}

#[derive(Deserialize)]
struct DeleteRefBody {
    name: String,
}

async fn delete_ref(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Json(body): Json<DeleteRefBody>,
) -> Result<StatusCode, ApiError> {
    let principal = write_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::RefDelete, None)?;
    if body.name == repo.default_ref {
        return Err(ApiError::bad("cannot delete the default ref"));
    }
    // Soft-delete by recording an audit row. The ref row stays until a later
    // maintenance pass; clients treat a missing listing filter as live.
    let _ = state.app.store.audit(
        principal.user_id,
        via_name(&principal),
        "ref.delete",
        Some(repo.id),
        &body.name,
        "allowed",
        "recorded; physical delete is not implemented",
    );
    Err(ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "not_implemented",
        "deleting a ref is recorded in the audit log and not yet applied",
    ))
}

#[derive(Deserialize)]
struct RevQuery {
    #[serde(rename = "ref")]
    rev: Option<String>,
    path: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<u32>,
}

fn resolve(app: &App, repo: &RepoRef, rev: Option<&str>) -> Result<ChangesetId, ApiError> {
    let rev = rev
        .filter(|s| !s.is_empty())
        .unwrap_or(repo.default_ref.as_str());
    if let Ok(id) = ChangesetId::parse_hex(rev) {
        if app.store.get_changeset(&repo.ns, id)?.is_some() {
            return Ok(id);
        }
    }
    match app.store.get_ref(&repo.ns, rev)? {
        Some(state) => Ok(state.target),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "empty",
            "repository has no commits on that ref",
        )),
    }
}

async fn tree(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Query(query): Query<RevQuery>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    let path = query.path.filter(|p| !p.is_empty());
    auth_repo(
        &state.app,
        &principal,
        &repo,
        Action::ObjectRead,
        path.as_deref(),
    )?;
    let tip = resolve(&state.app, &repo, query.rev.as_deref())?;
    let dir = path
        .as_deref()
        .map(|p| RepoPath::new(p).map_err(|err| ApiError::bad(err.to_string())))
        .transpose()?;
    let entries = state.app.engine.list_dir(&repo.ns, tip, dir.as_ref())?;
    let mut visible = Vec::new();
    for (entry_name, entry) in entries {
        let full = match &path {
            Some(prefix) => format!("{prefix}/{entry_name}"),
            None => entry_name.clone(),
        };
        let Some(user_id) = principal.user_id else {
            continue;
        };
        if state
            .app
            .accounts
            .path_denied(user_id, repo.id, &full, false)?
        {
            continue;
        }
        visible.push(entry_json(&entry_name, &entry));
    }
    Ok(Json(
        json!({"ref": query.rev.unwrap_or(repo.default_ref), "changeset": tip.to_hex(), "path": path.unwrap_or_default(), "entries": visible}),
    ))
}

fn entry_json(name: &str, entry: &scion_core::TreeEntry) -> Value {
    match entry {
        scion_core::TreeEntry::Dir { tree } => {
            json!({"name": name, "type": "dir", "tree": tree.to_hex()})
        }
        scion_core::TreeEntry::File { kind, blob, size } => json!({
            "name": name,
            "type": "file",
            "kind": match kind {
                FileKind::Regular => "regular",
                FileKind::Executable => "executable",
                FileKind::Symlink => "symlink",
            },
            "blob": blob.to_hex(),
            "size": size,
        }),
    }
}

async fn file(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Query(query): Query<RevQuery>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    let Some(path) = query.path else {
        return Err(ApiError::bad("path is required"));
    };
    auth_repo(
        &state.app,
        &principal,
        &repo,
        Action::ObjectRead,
        Some(&path),
    )?;
    let tip = resolve(&state.app, &repo, query.rev.as_deref())?;
    let repo_path = RepoPath::new(&path).map_err(|err| ApiError::bad(err.to_string()))?;
    let Some(bytes) = state.app.engine.read_file(&repo.ns, tip, &repo_path)? else {
        return Err(ApiError::not_found());
    };
    let text = String::from_utf8(bytes.clone()).ok();
    let binary = text.is_none() || bytes.contains(&0);
    Ok(Json(json!({
        "path": path,
        "changeset": tip.to_hex(),
        "size": bytes.len(),
        "binary": binary,
        "text": text,
        "base64": binary.then(|| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes)),
    })))
}

async fn log(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Query(query): Query<RevQuery>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::Query, None)?;
    let tip = resolve(&state.app, &repo, query.rev.as_deref())?;
    let limit = query.limit.unwrap_or(50).clamp(1, 500) as usize;
    let rows = state.app.engine.log(&repo.ns, tip, limit)?;
    Ok(Json(json!({
        "commits": rows.iter().map(changeset_view).collect::<Vec<_>>()
    })))
}

async fn blame(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Query(query): Query<RevQuery>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    let Some(path) = query.path.clone() else {
        return Err(ApiError::bad("path is required"));
    };
    auth_repo(&state.app, &principal, &repo, Action::Query, Some(&path))?;
    let tip = resolve(&state.app, &repo, query.rev.as_deref())?;
    let repo_path = RepoPath::new(&path).map_err(|err| ApiError::bad(err.to_string()))?;
    let blame = state.app.engine.blame(&repo.ns, tip, &repo_path)?;
    Ok(Json(json!({
        "path": path,
        "binary": blame.binary,
        "ranges": blame.ranges.iter().map(|range| json!({
            "start_line": range.start_line,
            "count": range.count,
            "origin": range.origin.to_hex(),
            "origin_start": range.origin_start,
        })).collect::<Vec<_>>(),
    })))
}

async fn diff(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Query(query): Query<RevQuery>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::Query, None)?;
    let to = if let Some(to) = &query.to {
        resolve(&state.app, &repo, Some(to))?
    } else {
        resolve(&state.app, &repo, query.rev.as_deref())?
    };
    let from = match &query.from {
        Some(from) => Some(resolve(&state.app, &repo, Some(from))?),
        None => state
            .app
            .store
            .get_changeset(&repo.ns, to)?
            .and_then(|cs| cs.first_parent()),
    };
    let files = state.app.engine.diff(&repo.ns, from, to)?;
    Ok(Json(json!({
        "from": from.map(|id| id.to_hex()),
        "to": to.to_hex(),
        "files": files.into_iter().map(|file| json!({
            "path": file.path,
            "status": file.status,
            "patch": file.patch,
        })).collect::<Vec<_>>(),
    })))
}

async fn history(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Query(query): Query<RevQuery>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    let Some(path) = query.path.clone() else {
        return Err(ApiError::bad("path is required"));
    };
    auth_repo(&state.app, &principal, &repo, Action::Query, Some(&path))?;
    let repo_path = RepoPath::new(&path).map_err(|err| ApiError::bad(err.to_string()))?;
    let limit = i64::from(query.limit.unwrap_or(50).clamp(1, 500));
    let ids = state.app.engine.path_history(&repo.ns, &repo_path, limit)?;
    let mut commits = Vec::new();
    for id in ids {
        if let Some(cs) = state.app.store.get_changeset(&repo.ns, id)? {
            commits.push(changeset_view(&cs));
        }
    }
    Ok(Json(json!({"path": path, "commits": commits})))
}

async fn landings(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::Query, None)?;
    let rows = state.app.store.list_landings(&repo.ns, 100)?;
    Ok(Json(json!({
        "landings": rows.into_iter().map(|row| json!({
            "ref": row.ref_name,
            "changeset": row.changeset.to_hex(),
            "landed_as": row.landed_as.to_hex(),
            "version": row.version,
            "ts": row.ts,
            "actor": row.actor,
        })).collect::<Vec<_>>()
    })))
}

async fn merge_base(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Query(query): Query<RevQuery>,
) -> Result<Json<Value>, ApiError> {
    let principal = read_user(&state.app, &headers)?;
    let repo = open_repo(&state.app, &owner, &name)?;
    auth_repo(&state.app, &principal, &repo, Action::Query, None)?;
    let Some(from) = query.from.as_deref() else {
        return Err(ApiError::bad("from is required"));
    };
    let Some(to) = query.to.as_deref() else {
        return Err(ApiError::bad("to is required"));
    };
    let a = resolve(&state.app, &repo, Some(from))?;
    let b = resolve(&state.app, &repo, Some(to))?;
    let base = state.app.engine.merge_base(&repo.ns, a, b)?;
    Ok(Json(json!({"merge_base": base.map(|id| id.to_hex())})))
}
