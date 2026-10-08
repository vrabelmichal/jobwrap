//! Shared build identity for the CLI, daemon, API, and browser interface.

/// Application version, also used by the Debian builder and release tag.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Full source commit, or `unknown` when built without Git metadata.
pub const GIT_COMMIT: &str = env!("JOBWRAP_GIT_COMMIT");
/// Whether the build included uncommitted source changes.
pub const BUILD_DIRTY: bool = env!("JOBWRAP_BUILD_DIRTY").as_bytes()[0] == b't';
/// Detailed output for `--version`; `-V` remains the short version.
pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\ncommit: ",
    env!("JOBWRAP_GIT_COMMIT"),
    "\nbuild dirty: ",
    env!("JOBWRAP_BUILD_DIRTY")
);
