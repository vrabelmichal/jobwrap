//! Token generation and hashing.
//!
//! API tokens are random 32-byte values encoded in base64url. Only a SHA-256
//! hash of the token is stored, so a database leak does not expose usable
//! tokens.

use rand::RngCore;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// The raw length of an API token in bytes.
const TOKEN_BYTES: usize = 32;

#[derive(Debug, Error)]
pub enum TokenError {
    #[error("random number generation failed")]
    Rng,
}

/// Generate a new opaque API token. Shown exactly once to the caller.
pub fn new_api_token() -> Result<String, TokenError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| TokenError::Rng)?;
    Ok(base64_url(&bytes))
}

/// Hash an API token for storage and lookup.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    hex(digest.as_slice())
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_generation_is_unique_and_hashable() {
        let a = new_api_token().expect("token");
        let b = new_api_token().expect("token");
        assert_ne!(a, b);
        assert_eq!(hash_token(&a), hash_token(&a));
        assert_ne!(hash_token(&a), hash_token(&b));
        // 32 bytes -> 43 base64url chars (no padding).
        assert_eq!(a.len(), 43);
    }
}
