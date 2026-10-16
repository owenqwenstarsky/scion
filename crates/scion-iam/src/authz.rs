//! The role/action table is the single source of truth for authorization.

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Role {
    None = 0,
    Read = 1,
    Write = 2,
    Maintain = 3,
    Admin = 4,
    Owner = 5,
}

impl Role {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => Role::None,
            "read" => Role::Read,
            "write" => Role::Write,
            "maintain" => Role::Maintain,
            "admin" => Role::Admin,
            "owner" => Role::Owner,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::None => "none",
            Role::Read => "read",
            Role::Write => "write",
            Role::Maintain => "maintain",
            Role::Admin => "admin",
            Role::Owner => "owner",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    ObjectRead,
    Query,
    ChangesetCreate,
    Land,
    ApplyEdits,
    RefCreate,
    RefDelete,
    RefProtect,
    SettingsWrite,
    MembersManage,
    PathRulesManage,
    TokenManageRepo,
    RepoDelete,
    RepoTransfer,
}

impl Action {
    pub const fn min_role(self) -> Role {
        match self {
            Action::ObjectRead | Action::Query => Role::Read,
            Action::ChangesetCreate | Action::Land | Action::ApplyEdits => Role::Write,
            Action::RefCreate | Action::RefDelete | Action::RefProtect | Action::SettingsWrite => {
                Role::Maintain
            }
            Action::MembersManage | Action::PathRulesManage | Action::TokenManageRepo => {
                Role::Admin
            }
            Action::RepoDelete | Action::RepoTransfer => Role::Owner,
        }
    }

    pub fn all() -> &'static [Action] {
        &[
            Action::ObjectRead,
            Action::Query,
            Action::ChangesetCreate,
            Action::Land,
            Action::ApplyEdits,
            Action::RefCreate,
            Action::RefDelete,
            Action::RefProtect,
            Action::SettingsWrite,
            Action::MembersManage,
            Action::PathRulesManage,
            Action::TokenManageRepo,
            Action::RepoDelete,
            Action::RepoTransfer,
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Via {
    Session,
    Token {
        max_role: Role,
        repos: Option<Vec<i64>>,
    },
    Service,
    Bootstrap,
}

#[derive(Clone, Debug)]
pub struct Principal {
    pub user_id: Option<i64>,
    pub username: String,
    pub site_admin: bool,
    pub via: Via,
    pub session_hash: Option<[u8; 32]>,
    pub csrf: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuthzError {
    NotFound,
    Forbidden,
    Unauthenticated,
}

pub fn decide(role: Role, action: Action, path_denied: bool) -> Result<(), AuthzError> {
    if role == Role::None {
        return Err(AuthzError::NotFound);
    }
    if role < action.min_role() {
        return Err(AuthzError::Forbidden);
    }
    if path_denied && action.min_role() >= Role::Read {
        return Err(AuthzError::Forbidden);
    }
    Ok(())
}

/// Tokens only narrow the caller's role. A token never widens access.
pub fn narrow(role: Role, via: &Via, repo_id: i64) -> Role {
    match via {
        Via::Token { max_role, repos } => {
            if let Some(list) = repos {
                if !list.contains(&repo_id) {
                    return Role::None;
                }
            }
            role.min(*max_role)
        }
        Via::Service | Via::Bootstrap => Role::None,
        Via::Session => role,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub role: String,
    pub can_read: bool,
    pub can_write: bool,
    pub can_land: bool,
    pub can_manage_refs: bool,
    pub can_manage_members: bool,
    pub can_admin_repo: bool,
    pub can_delete: bool,
}

pub fn capabilities(role: Role) -> Capabilities {
    let allow = |a: Action| role >= a.min_role();
    Capabilities {
        role: role.as_str().into(),
        can_read: allow(Action::ObjectRead),
        can_write: allow(Action::ChangesetCreate),
        can_land: allow(Action::Land),
        can_manage_refs: allow(Action::RefCreate),
        can_manage_members: allow(Action::MembersManage),
        can_admin_repo: allow(Action::PathRulesManage),
        can_delete: allow(Action::RepoDelete),
    }
}
