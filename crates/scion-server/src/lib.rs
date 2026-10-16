//! Scion server. The binary refuses to start outside a container and refuses
//! to start without credentials. The library can be embedded in tests.

mod app;
mod boot;
mod error;
mod guard;
mod http;
mod land;
mod tls;

pub use app::App;
pub use boot::{code, run, Exit};
pub use guard::{require_container, StartupError, SystemEnv};
pub use http::router;
