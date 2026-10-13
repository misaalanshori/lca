//! Internal lifecycle tracing and boundary inspection (gh #258).
//!
//! Always a dependency, never a cost: with the `debug-trace` feature
//! off (the stable line) every emitter compiles to an empty inline
//! body and neither `tracing` nor `tracing-subscriber` links in. With
//! it on (unstable and development builds) the domain emitters write
//! structured events to the two rotating debug files (see [`init`]).
//!
//! Nothing emits a secret: every value crossing into a trace goes
//! through [`redact`] first (AGENTS.md Rule 6).

pub mod core;
#[cfg(feature = "debug-trace")]
mod init;
#[cfg(feature = "debug-trace")]
pub use init::{init, subscriber, DebugConfig, INTERNAL_FILTER, WIRE_FILTER};
pub mod logfiles;
pub mod net;
pub mod permissions;
pub mod redact;
pub mod session;
pub mod wasm;

pub use logfiles::{DebugPaths, RotatingWriter, FILE_CAP_BYTES};

/// A trace domain: one `tracing` target, one of the two debug files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// Turn lifecycle, context budgeting, hooks, tools, steering, retry.
    Core,
    /// Append-only writes, lineage, branching, compaction.
    Session,
    /// Rule matching, trust, grants, yolo.
    Permissions,
    /// Outbound HTTP (wire file).
    Net,
    /// Wasmtime crossings (wire file).
    Wasm,
    /// Reference parsing, manifests, digests, lockfiles.
    Registry,
}

impl Domain {
    /// The `tracing` target (`LCA_LOG="lca_core=trace"`).
    pub fn target(self) -> &'static str {
        match self {
            Domain::Core => "lca_core",
            Domain::Session => "lca_session",
            Domain::Permissions => "lca_permissions",
            Domain::Net => "lca_net",
            Domain::Wasm => "lca_wasm",
            Domain::Registry => "lca_registry",
        }
    }

    /// Whether the domain writes the wire file (`debug-wire.log`)
    /// rather than the internal file (`debug.log`).
    pub fn is_wire(self) -> bool {
        matches!(self, Domain::Net | Domain::Wasm)
    }
}

/// Emit one structured event. Off-feature this is an empty inline body
/// (zero symbols, zero overhead); on-feature it writes a `tracing`
/// event at the domain's target with the event name and detail.
#[inline(always)]
pub fn emit(domain: Domain, name: &str, detail: &str) {
    emit_inner(domain, name, detail);
}

#[cfg(feature = "debug-trace")]
fn emit_inner(domain: Domain, name: &str, detail: &str) {
    // The `target:` fragment takes a literal, so one arm per domain.
    match domain {
        Domain::Core => tracing::info!(target: "lca_core", event = name, detail = %detail),
        Domain::Session => tracing::info!(target: "lca_session", event = name, detail = %detail),
        Domain::Permissions => {
            tracing::info!(target: "lca_permissions", event = name, detail = %detail)
        }
        Domain::Net => tracing::info!(target: "lca_net", event = name, detail = %detail),
        Domain::Wasm => tracing::info!(target: "lca_wasm", event = name, detail = %detail),
        Domain::Registry => tracing::info!(target: "lca_registry", event = name, detail = %detail),
    }
}

#[cfg(not(feature = "debug-trace"))]
#[inline(always)]
fn emit_inner(_domain: Domain, _name: &str, _detail: &str) {}
