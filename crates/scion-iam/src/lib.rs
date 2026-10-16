mod accounts;
mod authz;
mod password;

pub use accounts::{namespace_for, Accounts, LoginResult, RepoRow, TokenInfo, UserRow};
pub use authz::{
    capabilities, decide, narrow, Action, AuthzError, Capabilities, Principal, Role, Via,
};
pub use password::{hash_password, validate_password, verify_password, PasswordParams};

use scion_store::Error as StoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] scion_core::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("the server already has an administrator")]
    AlreadyBootstrapped,
    #[error("not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("{0}")]
    Msg(String),
}
