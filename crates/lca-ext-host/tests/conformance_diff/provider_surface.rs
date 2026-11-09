//! The provider-adjacent worlds in both delivery modes: a typed image block, the login surface, and declared structured output (ADR-0029/0033, gh #49).
//!
//! Gh #552 split `tests/conformance_diff.rs` (it was 1,180 lines, 20 under gate
//! 11's ceiling) at the world groups it already had, following the pattern its
//! two earlier siblings established (`args.rs`, `serve_http.rs`). The dual-mode
//! harness - `Fixture`, `both_modes`, `both_modes_keeping_wasm` - stays in the
//! barrel and is reached through `super`: it is what every row calls, and a copy
//! per file would recompile the ~6 s WASM fixture once more per row
//! (`tests/regressions/449`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::Fixture;
use super::args::call;
// Verifies: ADR-0029 and NFR-25 - a typed image content block crosses the
// ABI byte for byte in both delivery modes. This is the window's first
// breaking change: `message.content` went from a joined string to a list of
// `content-block`s, and the native twin and the WASM component must agree.
#[tokio::test]
async fn image_content_round_trips_identically_across_modes() {
    let fixture = Fixture::new("diff-image");
    let (wasm, native, _) = fixture.both_modes();
    // A tiny PNG-shaped byte string: the point is byte-exact transport, not a
    // decodable image.
    let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4];
    let message = lca_protocol::ChatMessage {
        role: lca_protocol::MessageRole::User,
        content: vec![
            lca_protocol::ContentBlock::Text {
                text: "what is in this image?".to_string(),
            },
            lca_protocol::ContentBlock::Image {
                media_type: "image/png".to_string(),
                bytes: png.clone(),
            },
        ],
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    };
    let wasm_out = wasm
        .transform_messages(vec![message.clone()])
        .await
        .expect("wasm transform")
        .expect("no rejection");
    let native_out = native
        .transform_messages(vec![message.clone()])
        .await
        .expect("native transform")
        .expect("no rejection");
    assert_eq!(wasm_out, native_out, "the image survives identically");
    assert_eq!(
        wasm_out,
        vec![message],
        "the echo transform returns the image block unchanged"
    );
}

// Verifies: ADR-0033 (the provider login surface agrees across delivery
// modes: options round-trip and a submit returns the same opaque settings).
#[tokio::test]
async fn the_login_surface_agrees_across_modes() {
    let fixture = Fixture::new("login");
    let (wasm, native, _engine) = fixture.both_modes();

    let wasm_options = wasm.login_options().await.expect("wasm options");
    let native_options = native.login_options().await.expect("native options");
    assert_eq!(wasm_options, native_options, "identical login options");
    assert_eq!(wasm_options.len(), 2, "the probe reports two options");
    assert_eq!(wasm_options[0].kind, "api-key");

    let answer = lca_protocol::LoginAnswer {
        choice: "conformance".to_string(),
        values: [("api-key".to_string(), "secret".to_string())]
            .into_iter()
            .collect(),
    };
    let wasm_settings = wasm
        .login_submit(answer.clone())
        .await
        .expect("wasm submit");
    let native_settings = native.login_submit(answer).await.expect("native submit");
    assert_eq!(wasm_settings, native_settings, "identical settings");
    assert!(
        wasm_settings.iter().any(|(key, _)| key == "base_url"),
        "the settings the host persists came back"
    );
}

// Verifies: #49 (EFG-017) - declared structured output in both delivery
// modes: the model gets text, the programmatic path gets the
// `structured_content` data verbatim, and an error keeps its data too.
// The schema declares the output shape in both modes.
#[tokio::test]
async fn structured_output_round_trips_in_both_modes() {
    let fixture = Fixture::new("structured");
    // gh #449: keep the concrete guest handle, so the schema assertions
    // below reuse this one compile instead of paying for more.
    let (concrete, wasm, native, _) = fixture.both_modes_keeping_wasm();
    for (label, args, status, text) in [
        (
            "ok",
            r#"{"mode":"structured"}"#,
            lca_protocol::ToolResultStatus::Ok,
            "the answer is 42 parsecs",
        ),
        (
            "err",
            r#"{"mode":"structured","ok":false}"#,
            lca_protocol::ToolResultStatus::Error,
            "the sensor failed, but here is what it reported",
        ),
    ] {
        let wasm_result = wasm.execute_tool(&call(args)).await.expect("wasm");
        let native_result = native.execute_tool(&call(args)).await.expect("native");
        for (side, result) in [("wasm", &wasm_result), ("native", &native_result)] {
            assert_eq!(result.status, status, "{label}/{side} status");
            assert_eq!(result.content, text, "{label}/{side} model text");
            assert_eq!(
                result.extras.get("structured_content").map(String::as_str),
                Some(r#"{"answer":42,"unit":"parsecs"}"#),
                "{label}/{side} data verbatim"
            );
        }
        assert_eq!(
            wasm_result.extras, native_result.extras,
            "{label} modes agree on data"
        );
    }
    // The single-tool schema declares the output shape in both modes.
    // (The suite dispatch prefers catalog specs; the declaration
    // lives on the single-tool schema both twins share.)
    //
    // gh #449: the guest is asked for its schema on the handle loaded
    // above, not on a third `host.load`. `schema()` is a fresh call
    // into the same component, so a second compile proves nothing
    // about the declaration - it only paid `Component::new` again.
    {
        let spec = concrete.schema().expect("wasm schema");
        assert!(
            spec.extras
                .get("output-schema")
                .is_some_and(|schema| schema.contains("answer")),
            "wasm schema declares the output shape"
        );
    }
    {
        let native = conformance::NativeConformance::new(fixture.capabilities());
        let spec = native.schema();
        assert!(
            spec.extras
                .get("output-schema")
                .is_some_and(|schema| schema.contains("answer")),
            "native schema declares the output shape"
        );
    }
}
