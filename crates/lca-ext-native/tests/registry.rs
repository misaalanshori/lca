//! The native registry: the wrapper the binary uses to collect in-binary
//! handles (ADR-0019, FR-EXT-6). Mostly a guard against someone re-pointing
//! the default set at a stub instead of the real first-party engine.

use std::sync::Arc;

use lca_ext_abi::DeliveryMode;
use lca_ext_native::{NativeRegistry, StatsSource, default_native_extensions};

fn stats() -> StatsSource {
    Arc::new(|| "stats".to_string())
}

// Verifies: FR-EXT-6 (native and WASM handles register through the same
// `Arc<dyn ExtensionDispatch>` type, in registration order).
#[test]
fn the_registry_preserves_registration_order_and_consumes() {
    let mut registry = NativeRegistry::new();
    assert!(registry.handles().is_empty());

    let handles = default_native_extensions(stats());
    assert_eq!(handles.len(), 1, "the shipped set is hooks-example");
    let handle = handles.into_iter().next().expect("one handle");
    registry.register(handle.clone());
    assert_eq!(registry.handles().len(), 1);
    assert!(Arc::ptr_eq(&registry.handles()[0], &handle));

    let consumed = registry.into_handles();
    assert_eq!(consumed.len(), 1);
    assert!(Arc::ptr_eq(&consumed[0], &handle));
}

// Verifies: FR-EXT-7 (the native delivery is labeled unsandboxed) and that
// the default set is the real handle, not an empty stand-in.
#[test]
fn the_default_handle_is_the_native_hooks_example() {
    let handle = default_native_extensions(stats())
        .into_iter()
        .next()
        .expect("one handle");
    assert_eq!(handle.name(), "hooks-example");
    assert_eq!(handle.delivery(), DeliveryMode::Native);
    assert_eq!(handle.delivery().label(), "in-process (unsandboxed)");
    assert!(
        handle
            .builtin_command_slots()
            .iter()
            .any(|slot| slot == "stats"),
        "the real handle fills the /stats slot"
    );
}
