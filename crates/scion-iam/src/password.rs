use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};
use scion_core::Error as CoreError;

#[derive(Clone, Debug)]
pub struct PasswordParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub lanes: u32,
}

impl Default for PasswordParams {
    fn default() -> Self {
        Self {
            // OWASP's published argon2id minimum. Tests lower this via configuration.
            memory_kib: 19 * 1024,
            iterations: 2,
            lanes: 1,
        }
    }
}

impl PasswordParams {
    pub fn fast_for_tests() -> Self {
        Self {
            memory_kib: 32,
            iterations: 1,
            lanes: 1,
        }
    }

    fn argon2(&self) -> Argon2<'static> {
        let params = Params::new(self.memory_kib, self.iterations, self.lanes, None)
            .expect("argon2 parameters");
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
    }
}

const COMMON: &[&str] = &[
    "password",
    "password123",
    "password1234",
    "123456789012",
    "qwertyuiop12",
    "letmeinplease",
    "changeme1234",
    "adminadmin12",
];

pub fn validate_password(password: &str) -> Result<(), CoreError> {
    if password.len() < 12 || password.len() > 128 {
        return Err(CoreError::Invalid(
            "password must be 12 to 128 characters".into(),
        ));
    }
    if COMMON.iter().any(|c| password.eq_ignore_ascii_case(c)) {
        return Err(CoreError::Invalid("password is too common".into()));
    }
    Ok(())
}

pub fn hash_password(password: &str, params: &PasswordParams) -> Result<String, CoreError> {
    validate_password(password)?;
    params
        .argon2()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| CoreError::Other(format!("argon2: {e}")))
}

pub fn verify_password(password: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn needs_rehash(phc: &str, params: &PasswordParams) -> bool {
    fn param(phc: &str, key: &str) -> Option<u32> {
        for part in phc.split('$') {
            for kv in part.split(',') {
                let mut it = kv.split('=');
                if it.next() == Some(key) {
                    return it.next()?.parse().ok();
                }
            }
        }
        None
    }
    let mem = param(phc, "m").unwrap_or(0);
    let time = param(phc, "t").unwrap_or(0);
    mem < params.memory_kib || time < params.iterations
}
