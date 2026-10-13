//! Wasmtime crossings — host imports and guest exports (gh #258
//! §3E): the `lca_wasm` domain's trace points (the wire file).
//! Parameters crossing in are redacted first (`crate::redact`); with
//! the `debug-trace` feature off each call compiles to nothing.

/// This domain's `tracing` target.
pub const TARGET: &str = "lca_wasm";

/// One sandbox-crossing event (`name` like `"import.fs.read"`).
#[inline(always)]
pub fn event(name: &str, detail: &str) {
    crate::emit(crate::Domain::Wasm, name, detail);
}
