//! Protocol notes.
//!
//! The running server exposes the JSON façade in `scion-server` (`/v1/...`).
//! `proto/scion/v1/scion.proto` records the gRPC shape from the plan. Tonic is
//! not mounted on the listener in this build: the CLI and the web BFF both
//! speak JSON over the published TLS port, which is the tested contract.

/// Domain used when hashing a bootstrap token.
pub const BOOTSTRAP_DOMAIN: &str = "scion bootstrap v1";
/// Domain used when hashing the web service token.
pub const WEB_SERVICE_DOMAIN: &str = "scion web service v1";
