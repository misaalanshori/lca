//! Domain wiring (gh #258 §3, §5): targets match the `LCA_LOG`
//! vocabulary, and wire domains route to the wire file.

use lca_debug::Domain;

// Verifies: gh #258 - every module's target is the `LCA_LOG` name.
#[test]
fn targets_match_the_log_vocabulary() {
    assert_eq!(lca_debug::core::TARGET, Domain::Core.target());
    assert_eq!(lca_debug::session::TARGET, Domain::Session.target());
    assert_eq!(lca_debug::permissions::TARGET, Domain::Permissions.target());
    assert_eq!(lca_debug::net::TARGET, Domain::Net.target());
    assert_eq!(lca_debug::wasm::TARGET, Domain::Wasm.target());
    assert_eq!(Domain::Core.target(), "lca_core");
    assert_eq!(Domain::Net.target(), "lca_net");
    assert_eq!(Domain::Wasm.target(), "lca_wasm");
}

// Verifies: gh #258 - only net and wasm write the wire file.
#[test]
fn wire_routing_covers_net_and_wasm_only() {
    assert!(Domain::Net.is_wire());
    assert!(Domain::Wasm.is_wire());
    assert!(!Domain::Core.is_wire());
    assert!(!Domain::Session.is_wire());
    assert!(!Domain::Permissions.is_wire());
    assert!(!Domain::Registry.is_wire());
}

// Verifies: gh #258 - emitting without the feature is a silent no-op
// (no panic, no output, nothing to initialize).
#[test]
fn emission_without_the_feature_is_a_silent_no_op() {
    lca_debug::core::event("turn.start", "id=abc");
    lca_debug::session::event("record.append", "type=user");
    lca_debug::permissions::event("rule.deny", "pattern=/etc/*");
    lca_debug::net::event("http.request", "GET https://x.test/");
    lca_debug::wasm::event("import.fs.read", "scope=workspace");
    lca_debug::emit(Domain::Registry, "ref.parse", "scheme=oci");
}
