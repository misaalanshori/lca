//! Gh #314: the WASM provider guest omitted two `ProviderCap` delegations,
//! so the WASM path silently did less than the native path.
//!
//! `GuestCap` (the shared `subscription_wasm_dispatcher!` macro's capability
//! view) implemented the net request/status/body/close calls and the three
//! credential calls - and stopped. Both remaining methods have trait defaults
//! that *compile* and *succeed quietly*:
//!
//! - `net_response_headers` defaulted to `Ok(Vec::new())`, so a WASM
//!   codex/grok turn never saw `retry-after`. Native honours it for
//!   server-directed backoff (gh #52); WASM could not, and an empty header
//!   list reads to a caller as "this response has none".
//! - `resource_read` defaulted to `NotFound`, so a WASM provider could not
//!   read its own `resources/` bag at all - presets, curated model lists, any
//!   data-only package content. The `lca:host/resources` import was already
//!   bound in the provider world; only the delegation was missing.
//!
//! The behavioral proof lives beside the code, where it can run the real
//! guest: `crates/lca-ext-host/tests/provider.rs`'s
//! `the_provider_capability_delegations_cross_in_both_modes` drives the
//! conformance component and compares it with the native twin -- measured
//! red by reverting the delegations (`resource=<resource failed: not found:
//! no resource probe/provider.txt>` against the native `resource=provider-
//! resource-ok`).
//!
//! These rows pin the *reason* the omission was invisible, so the next trait
//! method added with a quiet default gets the same scrutiny.
//!
//! Verifies: gh #314 acceptance 1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// Verifies: gh #314 - both trait defaults are quiet successes, which is the
// class cause. If a default ever changes to an error, an omission becomes
// loud by construction and these guards are redundant - so this row failing
// is a prompt to re-read the trait, not a bug.
#[test]
fn the_trait_defaults_are_quiet_successes() {
    use lca_protocol::ProviderCap;

    /// The smallest possible capability view: it implements only what the
    /// trait demands, exactly as a guest that forgot the two methods would
    /// behave by inheriting them.
    struct Bare;
    impl ProviderCap for Bare {
        fn net_request(
            &self,
            _method: &str,
            _url: &str,
            _headers: &[(&str, &str)],
            _body: Option<&[u8]>,
        ) -> Result<u32, lca_protocol::CapabilityError> {
            Ok(0)
        }
        fn net_response_status(&self, _handle: u32) -> Result<u16, lca_protocol::CapabilityError> {
            Ok(200)
        }
        fn net_read_body(
            &self,
            _handle: u32,
            _max: usize,
        ) -> Result<Option<Vec<u8>>, lca_protocol::CapabilityError> {
            Ok(None)
        }
        fn net_close_response(&self, _handle: u32) -> Result<(), lca_protocol::CapabilityError> {
            Ok(())
        }
        fn credentials_get(&self, _key: &str) -> Option<String> {
            None
        }
        fn credentials_set(
            &self,
            _key: &str,
            _value: &str,
        ) -> Result<(), lca_protocol::CapabilityError> {
            Ok(())
        }
        fn credentials_delete(&self, _key: &str) -> Result<(), lca_protocol::CapabilityError> {
            Ok(())
        }
    }

    // The header default: an empty list, indistinguishable from "no
    // headers on this response".
    assert_eq!(
        Bare.net_response_headers(1).expect("the default succeeds"),
        Vec::new(),
        "the header default is a quiet empty success - this is why the omission was silent"
    );
    // The resource default: NotFound for every path, indistinguishable from
    // a genuinely missing resource.
    let err = Bare
        .resource_read("presets.toml")
        .expect_err("the resource default refuses");
    assert!(
        matches!(err, lca_protocol::CapabilityError::NotFound(_)),
        "the resource default is NotFound (silent), got {err:?}"
    );
}

// Verifies: gh #314 - the provider world binds the resources import, so the
// fix needed no WIT change: the capability was already reachable, only
// un-delegated. A guest cannot call an import its world does not declare, so
// this row names the coupling the delegation depends on.
#[test]
fn the_provider_world_imports_resources() {
    let wit = include_str!("../../wit/world-provider.wit");
    assert!(
        wit.contains("import lca:host/resources"),
        "the provider world must declare the resources import"
    );
}
