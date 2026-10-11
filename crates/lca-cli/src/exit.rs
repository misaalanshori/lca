//! Process exit codes from `docs/headless.md`. Split from `lib.rs`
//! for the workspace file ceiling (gate 11, #158). Behaviour unchanged.

/// The turn completed.
pub const OK: i32 = 0;
/// Unexpected internal error.
pub const INTERNAL: i32 = 1;
/// Usage error: bad flags or arguments.
pub const USAGE: i32 = 2;
/// Provider error after the retry limit.
pub const PROVIDER: i32 = 3;
/// Permission denied: an action needed approval and headless mode
/// cannot prompt.
pub const PERMISSION: i32 = 4;
/// Turn aborted: iteration limit or a rejection from an extension.
pub const ABORTED: i32 = 5;
/// Session error: missing, malformed, or another project's session.
pub const SESSION: i32 = 6;
