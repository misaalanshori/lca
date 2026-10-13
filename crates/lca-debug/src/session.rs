//! Append-only writes, lineage, branching, and compaction (gh #258
//! §3B): the `lca_session` domain's trace points. Every value crossing
//! in is redacted first (`crate::redact`); with the `debug-trace`
//! feature off each call compiles to nothing.

/// This domain's `tracing` target.
pub const TARGET: &str = "lca_session";

/// One session-engine event (`name` like `"record.append"`).
#[inline(always)]
pub fn event(name: &str, detail: &str) {
    crate::emit(crate::Domain::Session, name, detail);
}
