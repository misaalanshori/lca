//! Rule matching, trust checks, and grant lifecycle (gh #258 §3C):
//! the `lca_permissions` domain's trace points. Actions log by display
//! shape (never a raw path with secrets beside it); with the
//! `debug-trace` feature off each call compiles to nothing.

/// This domain's `tracing` target.
pub const TARGET: &str = "lca_permissions";

/// One permission-evaluation event (`name` like `"rule.deny"`).
#[inline(always)]
pub fn event(name: &str, detail: &str) {
    crate::emit(crate::Domain::Permissions, name, detail);
}
