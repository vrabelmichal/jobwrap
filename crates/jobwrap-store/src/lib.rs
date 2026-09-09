//! jobwrap-store: SQLite repositories, migrations, append-only logs and
//! retention.

#![forbid(unsafe_code)]

mod logfile;
mod migrations;
mod models;
mod store;

pub use logfile::{delete_log, total_log_bytes, LogLimits, OutputLog};
pub use models::{parse_scope, StoredSession, StoredToken, TokenScope};
pub use store::{HelpCacheEntry, Store, StoreError};
