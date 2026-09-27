//! The terminal engine and widget library: the pi TUI port (ADR-0036).
//!
//! Agent-agnostic by the crate boundary rule: this crate may not import
//! `lca-core`, `lca-protocol`, `lca-session`, or session types. The agent
//! interface lives in `lca-ui`.

#![deny(unsafe_code)]

pub mod engine;
pub mod widgets;
