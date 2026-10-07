//! GitHub issue #179: the antigravity 0.9.0 wire alignment.
//!
//! Four divergences caused auth deadlocks, fingerprint mismatches, and
//! 400s on tool calls. The behavioral guards live with the extension
//! (`extensions/antigravity/tests/oauth.rs`, `tests/stream.rs`); this
//! file pins the wire-exactness invariants at the release-guard seam
//! (NFR-24), so a drift in the shared kit breaks the build by name.

use lca_wire_openai::responses::{ResponsesStream, build_responses_body};

// Verifies: gh #179 (divergence 2) - `stableUuid` matches pi-antigravity
// byte-for-byte; a bit-level drift would fingerprint differently.
#[test]
fn stable_uuid_matches_pi_antigravity() {
    assert_eq!(
        antigravity::stable_uuid("antigravity:exec:traj-1:1"),
        "f5315213-ed5b-59ce-bef4-30b5c216cb0d"
    );
}

// Verifies: gh #179 (divergence 3) - the bridge allowlist drops every
// rejected keyword: `nullable`, `anyOf`, `format`, `$ref`.
#[test]
fn the_normalizer_drops_every_rejected_keyword() {
    let hostile = serde_json::json!({
        "type": "object",
        "nullable": true,
        "format": "date",
        "properties": {
            "a": {"anyOf": [{"type": "string"}]},
            "b": {"$ref": "#/$defs/x"},
        },
        "$defs": {"x": {"type": "string"}},
    });
    let normalized = antigravity::normalize_custom_tool_schema(&hostile);
    let text = serde_json::to_string(&normalized).expect("serialize");
    for keyword in ["nullable", "anyOf", "format", "$ref", "$defs"] {
        assert!(!text.contains(keyword), "{keyword} survived: {text}");
    }
}

// Verifies: gh #179 (divergence 4) - the fallback table routes a 404ing
// preview id at the mapped backend model.
#[test]
fn the_fallback_table_routes_preview_ids_down() {
    assert_eq!(
        antigravity::fallback_runtime_model("gemini-3.8-flash", None),
        Some("gemini-3.7-flash-low".to_string())
    );
}

// Verifies: gh #179 (divergence 3) - the kit body pins the shared
// Responses shape: tools ride as function declarations, `store` stays
// false (these endpoints reject stored responses).
#[test]
fn the_responses_body_carries_tools_with_store_false() {
    let request = lca_protocol::CompletionRequest {
        messages: vec![
            lca_protocol::ChatMessage {
                role: lca_protocol::MessageRole::User,
                content: vec![lca_protocol::ContentBlock::Text {
                    text: "a".to_string(),
                }],
                tool_calls: Vec::new(),
                tool_call_id: None,
                usage: None,
                extras: Default::default(),
            },
            lca_protocol::ChatMessage {
                role: lca_protocol::MessageRole::User,
                content: vec![lca_protocol::ContentBlock::Text {
                    text: "b".to_string(),
                }],
                tool_calls: Vec::new(),
                tool_call_id: None,
                usage: None,
                extras: Default::default(),
            },
        ],
        tools: vec![lca_protocol::ToolSpec {
            name: "read".to_string(),
            description: "read".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            extras: Default::default(),
        }],
        model: "m".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let body = build_responses_body(&request, "", "m", None);
    assert_eq!(body["store"], false);
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["name"], "read");
    let mut stream = ResponsesStream::new();
    let completed = serde_json::json!({
        "type": "response.completed",
        "response": {"usage": {"input_tokens": 1, "output_tokens": 1}},
    });
    assert!(
        stream
            .feed(&completed)
            .iter()
            .any(|event| matches!(event, lca_protocol::StreamEvent::Usage { .. }))
    );
}
