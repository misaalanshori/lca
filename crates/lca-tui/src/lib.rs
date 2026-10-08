//! The terminal engine and widget library: the pi TUI port (ADR-0036).
//!
//! Agent-agnostic by the crate boundary rule: this crate may not import
//! `lca-core`, `lca-protocol`, `lca-session`, or session types. The agent
//! interface lives in `lca-ui`.

#![deny(unsafe_code)]

pub mod engine;
pub mod widgets;

/// The crash restore (#96): install on every entry path so a panic
/// leaves a usable terminal on every build profile.
pub use engine::crash::{CrashContext, set_crash_context, set_crash_extensions};
pub use engine::terminal::{
    install_panic_hook, install_panic_hook_with, panic_hook_installed, panic_restore_bytes,
};
