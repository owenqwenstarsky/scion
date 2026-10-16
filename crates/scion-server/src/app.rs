use crate::guard::{validate_credentials, AuthConfig, TlsMode};
use scion_core::hash_token;
use scion_derived::Engine;
use scion_iam::{Accounts, PasswordParams};
use scion_store::Store;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
pub struct App {
    pub store: Arc<Store>,
    pub engine: Arc<Engine>,
    pub accounts: Arc<Accounts>,
    pub bootstrap_hash: Option<[u8; 32]>,
    pub web_hash: Option<[u8; 32]>,
    pub started: Instant,
    pub plaintext: bool,
    pub version: &'static str,
}

impl App {
    pub fn open(
        dir: impl AsRef<Path>,
        bootstrap_token: Option<&str>,
        web_token: Option<&str>,
        argon: PasswordParams,
        plaintext: bool,
    ) -> Result<Arc<Self>, scion_store::Error> {
        let store = Arc::new(Store::open(dir)?);
        let engine = Arc::new(Engine::new(Arc::clone(&store)));
        let accounts = Arc::new(
            Accounts::new(Arc::clone(&store), argon)
                .map_err(|e| scion_store::Error::Msg(e.to_string()))?,
        );
        Ok(Arc::new(Self {
            store,
            engine,
            accounts,
            bootstrap_hash: bootstrap_token
                .map(|t| hash_token("scion bootstrap v1", t.as_bytes()).0),
            web_hash: web_token.map(|t| hash_token("scion web service v1", t.as_bytes()).0),
            started: Instant::now(),
            plaintext,
            version: env!("CARGO_PKG_VERSION"),
        }))
    }

    pub fn credential_gate(
        &self,
        bootstrap_token: Option<&[u8]>,
        tls: TlsMode,
        allow_plain: bool,
    ) -> Result<(), crate::guard::StartupError> {
        let stored = self.accounts.user_count().unwrap_or(0) as usize;
        validate_credentials(&AuthConfig {
            tls,
            allow_insecure_plaintext: allow_plain,
            bootstrap_token: bootstrap_token.map(|t| t.to_vec()),
            stored_credentials: stored,
            client_ca_configured: false,
        })
    }
}

pub fn token_eq(expected: &[u8; 32], presented: &str, domain: &str) -> bool {
    let got = hash_token(domain, presented.as_bytes()).0;
    subtle::ConstantTimeEq::ct_eq(expected.as_slice(), got.as_slice()).into()
}
