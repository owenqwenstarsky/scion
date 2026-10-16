use thiserror::Error;

/// Errors that originate in pure core logic (encoding, paths, invariants).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("not found")]
    NotFound,
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("encoding error: {0}")]
    Encoding(String),
    #[error("conflict on {0} paths")]
    Conflict(usize),
    #[error("compare-and-swap lost")]
    CasLost,
    #[error("not a fast-forward")]
    NotFastForward,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
