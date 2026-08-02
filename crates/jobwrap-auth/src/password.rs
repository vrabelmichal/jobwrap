//! Argon2id password hashing and verification.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("password hashing failed: {0}")]
    Hash(String),
    #[error("password is empty")]
    EmptyPassword,
}

/// Hash a password with Argon2id and a fresh random salt.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    if password.is_empty() {
        return Err(AuthError::EmptyPassword);
    }
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| AuthError::Hash(e.to_string()))?;
    Ok(hash.to_string())
}

/// Verify a password against a stored Argon2id hash.
pub fn verify_password(password: &str, stored: &str) -> Result<bool, AuthError> {
    let parsed = PasswordHash::new(stored).map_err(|e| AuthError::Hash(e.to_string()))?;
    let result = Argon2::default().verify_password(password.as_bytes(), &parsed);
    Ok(result.is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let hash = hash_password("hunter2").expect("hash");
        assert_ne!(hash, "hunter2");
        assert!(verify_password("hunter2", &hash).expect("verify"));
        assert!(!verify_password("wrong", &hash).expect("verify"));
    }

    #[test]
    fn rejects_empty() {
        assert!(hash_password("").is_err());
    }
}
