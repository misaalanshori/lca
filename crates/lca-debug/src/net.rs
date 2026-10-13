//! Outbound HTTP and the wire tap (gh #258 §3D): the `lca_net`
//! domain's trace points (the wire file). URLs pass through
//! `redact::redact_url`, headers through `redact::redact_header`,
//! bodies through `redact::redact_json`; with the `debug-trace`
//! feature off each call compiles to nothing.

/// This domain's `tracing` target.
pub const TARGET: &str = "lca_net";

/// One wire event (`name` like `"http.request"`).
#[inline(always)]
pub fn event(name: &str, detail: &str) {
    crate::emit(crate::Domain::Net, name, detail);
}
