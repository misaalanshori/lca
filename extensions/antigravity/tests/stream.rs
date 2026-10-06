//! The wire-alignment ports (gh #179, pi-antigravity 0.9.0 at
//! `a3d8cab`): deterministic labels, tool-schema normalization, and
//! fallback routing. Every shape here is asserted against the mock at
//! the wire level in `oauth.rs` journeys, not just below.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use antigravity::{
    build_request, fallback_runtime_model, normalize_custom_tool_schema, stable_uuid,
};
use lca_protocol::{CompletionRequest, ContentBlock, MessageRole};

// Verifies: gh #179 - `stableUuid` matches pi-antigravity
// `src/utils/util.ts` exactly (node and python oracles agree on these
// vectors; a bit-level drift would fingerprint differently).
#[test]
fn stable_uuid_matches_pi_antigravity() {
    assert_eq!(
        stable_uuid("antigravity:exec:traj-1:1"),
        "f5315213-ed5b-59ce-bef4-30b5c216cb0d"
    );
    assert_eq!(
        stable_uuid("antigravity:exec:traj-1:2"),
        "733c75c3-9ed0-5062-a97c-495377e3a18c"
    );
    assert_eq!(stable_uuid("hello"), "aaf4c61d-dcc5-58a2-9abe-de0f3b482cd9");
    assert_eq!(stable_uuid(""), "da39a3ee-5e6b-5b0d-b255-bfef95601890");
}

fn text_message(role: MessageRole, text: &str) -> lca_protocol::ChatMessage {
    lca_protocol::ChatMessage {
        role,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    }
}

fn two_turn_request(model: &str) -> CompletionRequest {
    CompletionRequest {
        messages: vec![
            text_message(MessageRole::User, "first"),
            text_message(MessageRole::Assistant, "answer"),
            text_message(MessageRole::User, "second"),
        ],
        tools: Vec::new(),
        model: model.to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

// Verifies: gh #179 - `last_execution_id` rides only past the first
// step, computed as `antigravity:exec:{trajectory}:{step-1}` over the
// request's own trajectory id.
#[test]
fn last_execution_id_rides_past_the_first_step() {
    let one_turn = CompletionRequest {
        messages: vec![text_message(MessageRole::User, "hi")],
        tools: Vec::new(),
        model: "gemini-3.8-flash-low".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let body = build_request(&one_turn, "", "p", "gemini-3.8-flash-low");
    assert!(
        body["request"]["labels"].get("last_execution_id").is_none(),
        "no label on the first step"
    );

    let body = build_request(
        &two_turn_request("gemini-3.8-flash-low"),
        "",
        "p",
        "gemini-3.8-flash-low",
    );
    let labels = &body["request"]["labels"];
    let trajectory = labels["trajectory_id"].as_str().expect("trajectory");
    assert_eq!(
        labels["last_execution_id"].as_str(),
        Some(stable_uuid(&format!("antigravity:exec:{trajectory}:2")).as_str()),
        "step 3 points at execution 2"
    );
}

// Verifies: gh #179 - Gemini models carry `parametersJsonSchema` and no
// legacy `parameters`; Claude/GPT-OSS carry the normalized legacy
// `parameters` and no `parametersJsonSchema`.
#[test]
fn tool_channels_split_by_model_class() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {"path": {"type": "string", "description": "a file"}},
        "required": ["path"],
    });
    let request = |model: &str| CompletionRequest {
        messages: vec![text_message(MessageRole::User, "hi")],
        tools: vec![lca_protocol::ToolSpec {
            name: "read".to_string(),
            description: "read".to_string(),
            parameters: schema.clone(),
            extras: Default::default(),
        }],
        model: model.to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };

    let gemini = build_request(
        &request("gemini-3.8-flash-low"),
        "",
        "p",
        "gemini-3.8-flash-low",
    );
    let declaration = &gemini["request"]["tools"][0]["functionDeclarations"][0];
    assert!(
        declaration.get("parametersJsonSchema").is_some(),
        "gemini channel"
    );
    assert!(declaration.get("parameters").is_none(), "no legacy field");

    for model in ["claude-sonnet-4-6", "gpt-oss-120b"] {
        let runtime = if model.starts_with("claude-") {
            model
        } else {
            "gpt-oss-120b-medium"
        };
        let body = build_request(&request(model), "", "p", runtime);
        let declaration = &body["request"]["tools"][0]["functionDeclarations"][0];
        assert!(
            declaration.get("parameters").is_some(),
            "{model} legacy channel"
        );
        assert!(
            declaration.get("parametersJsonSchema").is_none(),
            "{model} no JsonSchema field"
        );
    }
}

// Verifies: gh #179 - the hostile schema (every keyword Cloud Code
// Assist's bridge rejects) normalizes to the allowlist: `nullable`,
// `anyOf`, `format`, `$ref`, and `$defs` go; `type` unions keep their
// first non-null scalar; non-string enums drop.
#[test]
fn the_hostile_schema_normalizes_to_the_allowlist() {
    let hostile = serde_json::json!({
        "type": ["string", "null"],
        "nullable": true,
        "format": "date-time",
        "description": "kept",
        "$defs": {"x": {"type": "string"}},
        "properties": {
            "choice": {"anyOf": [{"type": "string"}, {"type": "integer"}], "default": 1},
            "refed": {"$ref": "#/$defs/x"},
            "tags": {"type": "array", "items": {"type": "string", "format": "uri"}},
            "mode": {"enum": ["a", 1]},
            "extra": {"type": "object", "additionalProperties": true},
        },
        "required": ["choice"],
        "additionalProperties": false,
    });
    let normalized = normalize_custom_tool_schema(&hostile);
    assert_eq!(
        normalized,
        serde_json::json!({
            "type": "string",
            "description": "kept",
            "properties": {
                "choice": {},
                "refed": {},
                "tags": {"type": "array", "items": {"type": "string"}},
                "mode": {},
                "extra": {"type": "object"},
            },
            "required": ["choice"],
        })
    );
}

// Verifies: gh #179 - the fallback table ports
// `getFallbackRuntimeModel` row for row, prefix rules before exact
// rows, exactly pi's order.
#[test]
fn fallback_routing_matches_pi_antigravity() {
    // Prefix rewrites.
    assert_eq!(
        fallback_runtime_model("gemini-3.8-flash-tiered", None),
        Some("gemini-3.7-flash-tiered".to_string())
    );
    assert_eq!(
        fallback_runtime_model("gemini-3.7-flash-high", None),
        Some("gemini-3.6-flash-high".to_string())
    );
    // Exact rows.
    assert_eq!(
        fallback_runtime_model("gemini-3.8-flash", None),
        Some("gemini-3.7-flash-low".to_string())
    );
    assert_eq!(
        fallback_runtime_model("gemini-3.7-flash", None),
        Some("gemini-3.6-flash-low".to_string())
    );
    // The tiered row follows the effort.
    assert_eq!(
        fallback_runtime_model("gemini-3.7-flash-tiered", Some("medium")),
        Some("gemini-3.6-flash-medium".to_string())
    );
    assert_eq!(
        fallback_runtime_model("gemini-3.7-flash-tiered", Some("high")),
        Some("gemini-3.6-flash-high".to_string())
    );
    assert_eq!(
        fallback_runtime_model("gemini-3.7-flash-tiered", None),
        Some("gemini-3.6-flash-low".to_string())
    );
    // No fallback for stable models.
    assert_eq!(fallback_runtime_model("gemini-3.6-flash-low", None), None);
    assert_eq!(fallback_runtime_model("claude-sonnet-4-6", None), None);
}
