//! jobwrap-auth: password hashing, API token and session handling, rate
//! limiting, and token-scope evaluation.

#![forbid(unsafe_code)]

mod password;
mod ratelimit;
mod scopes;
mod session;
mod token;

pub use password::{hash_password, verify_password};
pub use ratelimit::RateLimiter;
pub use scopes::{token_to_grants, token_to_permissions};
pub use session::{new_session_token, session_hash};
pub use token::{hash_token, new_api_token};
