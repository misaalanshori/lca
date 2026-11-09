//! The conformance script in both delivery modes (FR-PROV-2/7/8, NFR-25): model
//! listing, the typed stream's event mapping, the orphan-delta error, and the
//! closed-receiver case.
//!
//! Gh #552 split `tests/provider.rs` at its own subjects; the `Fixture` is defined
//! once, in `support.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};

use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_protocol::StreamEvent;
use lca_provider::ToolCallAccumulator;

use super::support::{CloseAfter, Fixture, collect, request};

// Verifies: FR-PROV-2 — both modes list the same models, and they are
// the shared script's models (NFR-25 identity for model listing).
#[tokio::test]
async fn model_listing_is_identical_across_modes() {
    let fixture = Fixture::new("models");
    let (wasm, native) = fixture.both_modes();

    assert_eq!(wasm.worlds(), native.worlds());
    assert!(wasm.worlds().contains(&World::Provider));
    assert_eq!(wasm.delivery(), DeliveryMode::Wasm);
    assert_eq!(native.delivery(), DeliveryMode::Native);

    let wasm_models = wasm.provider_models(&[]).expect("wasm models");
    let native_models = native.provider_models(&[]).expect("native models");
    assert_eq!(wasm_models, native_models);
    assert_eq!(wasm_models, conformance::provider_models_default());

    // ADR-0035: the settings `login-submit` hands back are the source of
    // truth in BOTH modes - the discovered list shows up in each, and the
    // two agree.
    let discovered = [("models".to_string(), "found-a,found-b".to_string())];
    let wasm_discovered = wasm
        .provider_models(&discovered)
        .expect("wasm discovered")
        .into_iter()
        .map(|model| model.id)
        .collect::<Vec<_>>();
    let native_discovered = native
        .provider_models(&discovered)
        .expect("native discovered")
        .into_iter()
        .map(|model| model.id)
        .collect::<Vec<_>>();
    assert_eq!(wasm_discovered, native_discovered, "the two modes agree");
    assert_eq!(wasm_discovered, vec!["found-a", "found-b"]);
    assert_eq!(wasm_models.len(), 2);
    assert_eq!(wasm_models[0].id, "conformance-a");
    assert_eq!(wasm_models[1].context_window, 8192);
}

// Verifies: gh #316 - WASM `list_models` preserves `extras`, so the WASM
// path advertises the same vision/thinking/cache metadata the native path
// does. The pre-existing row above compares the two modes, but the fixture's
// models carried *empty* extras on both sides, so it agreed vacuously and
// could not see the guest's hardcoded `Vec::new()`.
#[tokio::test]
async fn model_extras_survive_the_wasm_round_trip() {
    let fixture = Fixture::new("model-extras");
    let (wasm, native) = fixture.both_modes();

    let wasm_models = wasm.provider_models(&[]).expect("wasm models");
    let native_models = native.provider_models(&[]).expect("native models");

    // The fixture must be exercising the field, or this row would repeat
    // the blind spot it exists to close.
    assert!(
        native_models.iter().any(|model| !model.extras.is_empty()),
        "the conformance fixture must publish models carrying extras"
    );
    assert_eq!(
        wasm_models, native_models,
        "NFR-25: the two modes must produce the same model metadata"
    );

    // And the specific metadata gh #316 reports as lost, named.
    let first = &wasm_models[0];
    assert_eq!(
        first.extras.get("vision").map(String::as_str),
        Some("true"),
        "the WASM path sees the vision flag"
    );
    assert_eq!(
        first.extras.get("thinking").map(String::as_str),
        Some("true"),
        "the WASM path sees the thinking flag"
    );
    assert_eq!(
        first.extras.get("prompt_cache").map(String::as_str),
        Some("short=300,long=3600"),
        "the WASM path sees the prompt-cache lifetimes"
    );
    // A model whose metadata differs keeps its own answer (no cross-talk).
    assert_eq!(
        wasm_models[1].extras.get("vision").map(String::as_str),
        Some("false")
    );
}

// Verifies: gh #314 - the two capability delegations `GuestCap` omitted.
// `resource_read` failed to the trait's `NotFound` default, so a WASM
// provider could not read its own `resources/` bag (presets, curated model
// lists); `net_response_headers` returned an empty list, so `retry-after`
// was invisible and a server-directed backoff never happened. Both ride the
// fixture's `usage` extras (`capability_probe`) so the two delivery modes
// are compared on one string.
#[tokio::test]
async fn the_provider_capability_delegations_cross_in_both_modes() {
    let fixture = Fixture::new("cap-delegations");
    let (wasm, native) = fixture.both_modes();

    let report = |usage: lca_protocol::Usage| {
        usage
            .extras
            .get("capability_probe")
            .cloned()
            .expect("the fixture's usage carries the probe report")
    };
    let wasm_report = report(
        wasm.identity_usage()
            .await
            .expect("wasm usage")
            .expect("wasm outcome"),
    );
    let native_report = report(
        native
            .identity_usage()
            .await
            .expect("native usage")
            .expect("native outcome"),
    );

    // The resource half is byte-identical across modes - that is the
    // gh #314 fix for `resource_read`. The header half differs only in how
    // an unissued handle is *named* (see below), so compare the halves.
    let resource_half = |report: &str| report.split(';').next().unwrap_or_default().to_string();
    assert_eq!(
        resource_half(&wasm_report),
        resource_half(&native_report),
        "NFR-25: the two modes read their resource bag identically"
    );
    assert!(
        wasm_report.contains(conformance::PROBE_RESOURCE_BODY),
        "the WASM guest read its own resource bag: {wasm_report}"
    );
    assert!(
        !wasm_report.contains("<resource failed"),
        "no resource failure in either mode: {wasm_report}"
    );
    // The delegation, not a default: an unissued handle must reach the host
    // and come back as a *failure*, because the trait default this fix
    // replaced answered `Ok(vec![])` - an empty header list, which is exactly
    // how the omission stayed invisible (a caller reads it as "this response
    // has no headers").
    //
    // The two modes spell the failure differently and that is not fixable
    // here: native says `not-found`, the WIT `net::error` variant has no
    // not-found case, so the host folds it into `invalid`. Recorded as an
    // ABI observation in this cycle's report rather than papered over - the
    // property that matters is that neither mode answers `Ok([])`.
    assert!(
        wasm_report.contains("headers=failed(unknown handle 0)"),
        "the header call reached the host with a real failure: {wasm_report}"
    );
    // The receipt, printed so a live run shows the values rather than only a
    // green tick (zero-spend: the conformance matrix IS the evidence - a real
    // subscription drive needs operator keys).
    println!("capability probe (wasm)   = {wasm_report}");
    println!("capability probe (native) = {native_report}");

    assert!(
        !wasm_report.contains("answered []"),
        "an empty header list would be the pre-fix default: {wasm_report}"
    );
}

// Verifies: ADR-0004's typed stream, NFR-25's exact-equality promise for
// every scripted event kind, FR-PROV-7's start-before-delta shape, and
// NFR-21 - the committed component's scripted output is a golden fixture, so
// a pinned-runtime upgrade that changes extension behavior fails here even
// inside one ABI version (the Wasmtime version is pinned in Cargo.lock).
#[tokio::test]
async fn streamed_events_match_the_script_and_each_other() {
    let fixture = Fixture::new("stream");
    let (wasm, native) = fixture.both_modes();

    for model in [
        "",
        "conformance-defaults",
        "conformance-tool",
        "conformance-vendor",
        "conformance-error",
    ] {
        let wasm_events = collect(wasm.as_ref(), model).await.expect("wasm stream");
        let native_events = collect(native.as_ref(), model)
            .await
            .expect("native stream");
        assert_eq!(
            wasm_events, native_events,
            "dual-mode divergence for model {model:?}"
        );
        assert_eq!(
            wasm_events,
            conformance::scripted_events(model),
            "host mapping lost fidelity for model {model:?}"
        );
    }

    // FR-PROV-7: the start event precedes every argument delta.
    let events = collect(wasm.as_ref(), "conformance-tool")
        .await
        .expect("tool stream");
    let mut saw_start = false;
    for event in &events {
        match event {
            StreamEvent::ToolCallStart { .. } => saw_start = true,
            StreamEvent::ToolCallArgDelta { .. } => assert!(saw_start, "delta before start"),
            _ => {}
        }
    }
    assert!(saw_start, "the tool script opened a call");
}

// Verifies: FR-PROV-8 — an argument delta with no open start is
// discarded by the host's accumulator and recorded as a protocol error.
#[tokio::test]
async fn orphan_deltas_become_protocol_errors() {
    let fixture = Fixture::new("orphan");
    let (wasm, _native) = fixture.both_modes();

    let events = collect(wasm.as_ref(), "conformance-orphan-delta")
        .await
        .expect("orphan stream");
    let mut accumulator = ToolCallAccumulator::default();
    for event in events {
        accumulator.handle(event);
    }
    let (calls, errors) = accumulator.finish(true);
    assert!(calls.is_empty(), "the orphan call never materializes");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        matches!(
            errors[0],
            lca_provider::ProtocolError::DeltaWithoutStart { ref call_id } if call_id == "ghost"
        ),
        "{errors:?}"
    );
}

// Verifies: FR-CONC-3 — closing the receiver ends the stream without
// disabling the extension (cancellation is not a defect by the
// extension, FR-EXT-3 does not fire).
#[tokio::test]
async fn a_closed_receiver_ends_the_stream_and_keeps_the_extension() {
    let fixture = Fixture::new("cancel");

    let wasm = fixture.wasm_mode();
    let sink = CloseAfter {
        events: Arc::new(Mutex::new(Vec::new())),
        budget: std::sync::atomic::AtomicUsize::new(1),
    };
    let _ = wasm
        .stream_completion(request("conformance-defaults"), &sink)
        .await;
    assert!(
        wasm.is_enabled(),
        "a receiver that walked away must not disable the extension"
    );

    let native = conformance::NativeConformance::new(fixture.capabilities());
    let sink = CloseAfter {
        events: Arc::new(Mutex::new(Vec::new())),
        budget: std::sync::atomic::AtomicUsize::new(1),
    };
    let _ = native
        .stream_completion(request("conformance-defaults"), &sink)
        .await;
}
