//! Turn lifecycle, context budgeting, hooks, tools, steering, and
//! retry (gh #258 §3A): the `lca_core` domain's trace points. Every
//! value crossing in is redacted first (`crate::redact`); with the
//! `debug-trace` feature off each call compiles to nothing.

/// This domain's `tracing` target.
pub const TARGET: &str = "lca_core";

/// One turn-lifecycle event (`name` like `"turn.start"`).
#[inline(always)]
pub fn event(name: &str, detail: &str) {
    crate::emit(crate::Domain::Core, name, detail);
}
