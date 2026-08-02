//! Browser session token generation.

use rand::RngCore;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Length of the opaque session token in bytes.
const SESSION_BYTES: usize = 32;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("random number generation failed")]
    Rng,
}

/// Generate a new opaque browser session token (stored in the cookie).
pub fn new_session_token() -> Result<String, SessionError> {
    let mut bytes = [0u8; SESSION_BYTES];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| SessionError::Rng)?;
    Ok(base64_url(&bytes))
}

/// Hash a session token for server-side storage.
pub fn session_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
