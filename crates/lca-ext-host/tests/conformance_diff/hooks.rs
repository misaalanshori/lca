//! The command, hook, and catalog surfaces in both delivery modes (FR-PROJ-*, FR-EXT-9, gh #45/#77).
//!
//! Gh #552 split `tests/conformance_diff.rs` (it was 1,180 lines, 20 under gate
//! 11's ceiling) at the world groups it already had, following the pattern its
//! two earlier siblings established (`args.rs`, `serve_http.rs`). The dual-mode
//! harness - `Fixture`, `both_modes`, `both_modes_keeping_wasm` - stays in the
//! barrel and is reached through `super`: it is what every row calls, and a copy
//! per file would recompile the ~6 s WASM fixture once more per row
//! (`tests/regressions/449`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_ext_abi::World;
use lca_protocol::{CommandEffect, HookAction, ToolCall};

use super::Fixture;
// Commands and hooks: same spec, same effects, same verdicts.
#[tokio::test]
async fn commands_and_hooks_agree_across_modes() {
    let fixture = Fixture::new("diff-hooks");
    let (wasm, native, _engine) = fixture.both_modes();

    assert_eq!(
        wasm.command_specs().expect("wasm"),
        native.command_specs().expect("native")
    );
    for argument in ["submit", "insert:hello", "", "unknown"] {
        assert_eq!(
            wasm.invoke_command("probe", argument).expect("wasm"),
            native.invoke_command("probe", argument).expect("native"),
            "command effect divergence for {argument:?}"
        );
    }
    assert_eq!(
        wasm.invoke_command("probe", "submit").expect("effect"),
        CommandEffect::SubmitPrompt("conformance submitted".to_string())
    );

    for tool in ["read", "probe-deny-tool", "shell"] {
        let call = ToolCall {
            call_id: "c9".to_string(),
            name: tool.to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        };
        let wasm_action = wasm.on_pre_tool_use(&call).await.expect("wasm");
        let native_action = native.on_pre_tool_use(&call).await.expect("native");
        assert_eq!(wasm_action, native_action, "hook divergence for {tool}");
    }
    let denied = wasm
        .on_pre_tool_use(&ToolCall {
            call_id: "c9".to_string(),
            name: "probe-deny-tool".to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        })
        .await
        .expect("hook");
    assert_eq!(
        denied,
        HookAction::Deny("conformance policy denied this tool".to_string())
    );

    // Observing hooks return Ok on both sides.
    let observation = lca_protocol::PostToolObservation {
        call: ToolCall {
            call_id: "c1".to_string(),
            name: "read".to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        },
        result: lca_protocol::ToolResult::ok("c1", "fine"),
    };
    wasm.on_post_tool_use(&observation).await.expect("wasm");
    native.on_post_tool_use(&observation).await.expect("native");
    wasm.on_pre_turn().await.expect("wasm");
    native.on_pre_turn().await.expect("native");
    wasm.on_post_turn_end("ok").await.expect("wasm");
    native.on_post_turn_end("ok").await.expect("native");
    wasm.on_session_close().await.expect("wasm");
    native.on_session_close().await.expect("native");

    assert!(wasm.worlds().contains(&World::Hooks));
}

// Verifies: gh #45 - every new hook point answers identically in both
// modes: marker-gated replace/mutate/compose plus inert defaults.
#[tokio::test]
async fn the_new_hooks_agree_across_modes() {
    let fixture = Fixture::new("hooks-new");
    let (wasm, native, _engine) = fixture.both_modes();

    // message_end replaces the marked token, observes the rest.
    for text in [
        "the token is conformance-secret, handle with care",
        "nothing to redact here",
    ] {
        assert_eq!(
            wasm.on_message_end("assistant", text).await.expect("wasm"),
            native
                .on_message_end("assistant", text)
                .await
                .expect("native"),
            "message_end agrees on {text:?}"
        );
    }
    assert_eq!(
        wasm.on_message_end(
            "assistant",
            "the token is conformance-secret, handle with care"
        )
        .await
        .expect("wasm"),
        Some("the token is conformance-redacted, handle with care".to_string())
    );

    // tool_call mutates the marked args, blocks the marked call.
    let plain = ToolCall {
        call_id: "c1".to_string(),
        name: "conformance".to_string(),
        arguments: r#"{"mode":"ok"}"#.to_string(),
        parent_call_id: None,
    };
    assert_eq!(
        wasm.on_tool_call(&plain).await.expect("wasm"),
        native.on_tool_call(&plain).await.expect("native"),
    );
    let mutate = ToolCall {
        arguments: r#"{"mode":"ok","tag":"tag-mutate"}"#.to_string(),
        ..plain.clone()
    };
    let wasm_patch = wasm.on_tool_call(&mutate).await.expect("wasm patch");
    assert_eq!(
        wasm_patch,
        native.on_tool_call(&mutate).await.expect("native patch"),
    );
    assert!(
        wasm_patch
            .arguments
            .expect("mutated")
            .contains("tag-mutated")
    );
    assert!(wasm_patch.block.is_none());
    let block = ToolCall {
        arguments: r#"{"tag":"tag-block"}"#.to_string(),
        ..plain.clone()
    };
    let wasm_block = wasm.on_tool_call(&block).await.expect("wasm block");
    assert_eq!(
        wasm_block,
        native.on_tool_call(&block).await.expect("native block")
    );
    assert_eq!(
        wasm_block.block.as_deref(),
        Some("conformance blocked this call")
    );

    // tool_result composes the marked content, observes the rest.
    let result = lca_protocol::ToolResult::ok("c1", "the token is conformance-secret!");
    let wasm_composed = wasm.on_tool_result(&plain, &result).await.expect("wasm");
    assert_eq!(
        wasm_composed,
        native
            .on_tool_result(&plain, &result)
            .await
            .expect("native"),
    );
    assert_eq!(
        wasm_composed.content.as_deref(),
        Some("the token is conformance-redacted!")
    );

    // Settle, compact-allow, and stream-observe are inert by default.
    for (wasm_d, native_d) in [
        (
            wasm.on_turn_end(2, 1, "ok").await.expect("wasm"),
            native.on_turn_end(2, 1, "ok").await.expect("native"),
        ),
        (
            wasm.on_agent_before_settle(2, 1, "ok").await.expect("wasm"),
            native
                .on_agent_before_settle(2, 1, "ok")
                .await
                .expect("native"),
        ),
    ] {
        assert_eq!(wasm_d, native_d);
        assert_eq!(wasm_d, lca_protocol::SettleDecision::default());
    }
    assert_eq!(
        wasm.on_session_before_compact("threshold")
            .await
            .expect("wasm"),
        native
            .on_session_before_compact("threshold")
            .await
            .expect("native"),
    );
    wasm.on_session_compact_failed("threshold", Some("boom"))
        .await
        .expect("wasm failed-hook");
    native
        .on_session_compact_failed("threshold", Some("boom"))
        .await
        .expect("native failed-hook");
    wasm.on_stream_event("p", "m", "text-delta", "hi")
        .await
        .expect("wasm stream");
    native
        .on_stream_event("p", "m", "text-delta", "hi")
        .await
        .expect("native stream");

    // Cache votes decline the marked model; trust votes decide the
    // marked paths and fall through everywhere else.
    for model in ["m", "model-no-warm"] {
        assert_eq!(
            wasm.on_cache_warming_decision("p", model)
                .await
                .expect("wasm"),
            native
                .on_cache_warming_decision("p", model)
                .await
                .expect("native"),
            "cache agrees on {model}"
        );
    }
    assert!(
        !(wasm
            .on_cache_warming_decision("p", "model-no-warm")
            .await
            .expect("wasm"))
    );
    for cwd in ["/tmp/work", "/tmp/trust-yes", "/tmp/trust-no"] {
        assert_eq!(
            wasm.on_project_trust(cwd).await.expect("wasm"),
            native.on_project_trust(cwd).await.expect("native"),
            "trust agrees on {cwd}"
        );
    }
    assert_eq!(
        wasm.on_project_trust("/tmp/trust-yes").await.expect("wasm"),
        (lca_protocol::TrustVote::Yes, true)
    );
}
