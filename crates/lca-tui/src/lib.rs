//! The terminal engine and widget library: the pi TUI port (ADR-0036).
//!
//! Agent-agnostic by the crate boundary rule: this crate may not import
//! `lca-core`, `lca-protocol`, `lca-session`, or session types. The agent
//! interface lives in `lca-ui`.

#![deny(unsafe_code)]
// R17: production paths stay panic-free (test code may unwrap; clippy.toml
// scopes the lints away from tests).
#![warn(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod engine;
pub mod widgets;
