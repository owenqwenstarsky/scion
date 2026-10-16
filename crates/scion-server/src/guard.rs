//! Startup guards. The container check is a guardrail, not a security boundary:
//! anyone can create `/.dockerenv` or set the environment variable. Real
//! protection is authentication, TLS, and network policy.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupError {
    NotInContainer,
    NoCredentials,
    PlaintextNotAcknowledged,
    WeakToken(usize),
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartupError::NotInContainer => f.write_str(
                "scion-server: refusing to start: not running inside a container.\n  \
                 The Scion server is supported only as a Docker container (image: scion-server).\n  \
                 Start it from the host with:   scion server up\n  \
                 Checked: /.dockerenv, SCION_IN_CONTAINER=1",
            ),
            StartupError::NoCredentials => f.write_str(
                "scion-server: refusing to start: no credentials configured.\n  \
                 Provide SCION_BOOTSTRAP_TOKEN_FILE (>= 32 bytes), an existing user database, or mTLS client CA.",
            ),
            StartupError::PlaintextNotAcknowledged => f.write_str(
                "scion-server: refusing to start: TLS is disabled without SCION_ALLOW_INSECURE_PLAINTEXT=1.",
            ),
            StartupError::WeakToken(n) => {
                write!(f, "scion-server: refusing to start: token has {n} bytes; need at least 32")
            }
        }
    }
}

pub trait Env {
    fn exists(&self, path: &str) -> bool;
    fn var(&self, key: &str) -> Option<String>;
}

pub struct SystemEnv;

impl Env for SystemEnv {
    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }

    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// Runs first in `main`, before the data directory is opened.
pub fn require_container(env: &dyn Env) -> Result<(), StartupError> {
    let marker = env.exists("/.dockerenv") || env.var("SCION_IN_CONTAINER").as_deref() == Some("1");
    if marker {
        Ok(())
    } else {
        Err(StartupError::NotInContainer)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    Required,
    Disabled,
}

#[derive(Clone, Debug)]
pub struct AuthConfig {
    pub tls: TlsMode,
    pub allow_insecure_plaintext: bool,
    pub bootstrap_token: Option<Vec<u8>>,
    pub stored_credentials: usize,
    pub client_ca_configured: bool,
}

pub fn validate_credentials(c: &AuthConfig) -> Result<(), StartupError> {
    if let Some(t) = &c.bootstrap_token {
        if t.len() < 32 {
            return Err(StartupError::WeakToken(t.len()));
        }
    }
    if c.bootstrap_token.is_none() && c.stored_credentials == 0 && !c.client_ca_configured {
        return Err(StartupError::NoCredentials);
    }
    if matches!(c.tls, TlsMode::Disabled) && !c.allow_insecure_plaintext {
        return Err(StartupError::PlaintextNotAcknowledged);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        dockerenv: bool,
        vars: Vec<(&'static str, &'static str)>,
    }

    impl Env for Fake {
        fn exists(&self, path: &str) -> bool {
            path == "/.dockerenv" && self.dockerenv
        }
        fn var(&self, key: &str) -> Option<String> {
            self.vars
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn container_guard_messages() {
        let bare = Fake {
            dockerenv: false,
            vars: vec![],
        };
        let err = require_container(&bare).unwrap_err();
        assert_eq!(err, StartupError::NotInContainer);
        assert!(err.to_string().contains("scion server up"));
        let flagged = Fake {
            dockerenv: false,
            vars: vec![("SCION_IN_CONTAINER", "1")],
        };
        assert!(require_container(&flagged).is_ok());
        let marked = Fake {
            dockerenv: true,
            vars: vec![],
        };
        assert!(require_container(&marked).is_ok());
    }

    #[test]
    fn credential_guard() {
        let weak = AuthConfig {
            tls: TlsMode::Required,
            allow_insecure_plaintext: false,
            bootstrap_token: Some(b"short".to_vec()),
            stored_credentials: 0,
            client_ca_configured: false,
        };
        assert!(matches!(
            validate_credentials(&weak),
            Err(StartupError::WeakToken(5))
        ));
        let none = AuthConfig {
            bootstrap_token: None,
            ..weak.clone()
        };
        assert_eq!(
            validate_credentials(&none),
            Err(StartupError::NoCredentials)
        );
        let plain = AuthConfig {
            tls: TlsMode::Disabled,
            allow_insecure_plaintext: false,
            bootstrap_token: Some(vec![1; 32]),
            stored_credentials: 0,
            client_ca_configured: false,
        };
        assert_eq!(
            validate_credentials(&plain),
            Err(StartupError::PlaintextNotAcknowledged)
        );
        let ok = AuthConfig {
            allow_insecure_plaintext: true,
            ..plain
        };
        assert!(validate_credentials(&ok).is_ok());
    }
}
