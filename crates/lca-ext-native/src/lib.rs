//! Native-linked extensions: registered exactly like WASM ones, through
//! `Arc<dyn ExtensionDispatch>` (ADR-0019, FR-EXT-6). No marshaling and
//! no sandbox, which is why the extension list labels these handles
//! `in-process (unsandboxed)` (FR-EXT-7, ADR-0013: first-party code
//! only).

#![forbid(unsafe_code)]

use std::sync::Arc;

use lca_ext_abi::ExtensionDispatch;

/// The handle type both delivery modes produce.
pub type NativeHandle = Arc<dyn ExtensionDispatch>;

/// Source of session statistics text for commands that display it.
pub type StatsSource = Arc<dyn Fn() -> String + Send + Sync>;

/// Collects the extensions compiled into this binary.
#[derive(Default)]
pub struct NativeRegistry {
    handles: Vec<NativeHandle>,
}

impl NativeRegistry {
    /// An empty registry.
    pub fn new() -> NativeRegistry {
        NativeRegistry::default()
    }

    /// Register one in-binary extension (collision checks belong to
    /// core's dispatch table, which owns ordering).
    pub fn register(&mut self, handle: NativeHandle) {
        self.handles.push(handle);
    }

    /// The registered handles, in registration order.
    pub fn handles(&self) -> &[NativeHandle] {
        &self.handles
    }

    /// Consume the registry into its handles.
    pub fn into_handles(self) -> Vec<NativeHandle> {
        self.handles
    }
}

/// The first-party extensions this binary ships compiled in
/// (`extensions/`, each behind its own crate). `stats` feeds the `/stats`
/// command's text; the Phase 2 move that proved the path was relocating
/// that built-in behavior out of the TUI and into `hooks-example`.
pub fn default_native_extensions(stats: StatsSource) -> Vec<NativeHandle> {
    vec![Arc::new(hooks_example::HooksExample::new(stats))]
}
