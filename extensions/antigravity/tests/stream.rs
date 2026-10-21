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
            exposure: lca_protocol::ToolExposure::Direct,
            namespace: None,
            annotations: None,
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

// ---------------------------------------------------------------------------
// gh #280: the system prompt must not ride in `contents` as a user turn
// ---------------------------------------------------------------------------

/// A system preamble plus one real user turn — the shape message assembly
/// actually sends.
fn system_then_user(system: &str, user: &str) -> CompletionRequest {
    CompletionRequest {
        messages: vec![
            text_message(MessageRole::System, system),
            text_message(MessageRole::User, user),
        ],
        tools: Vec::new(),
        model: "gemini-3.8-flash-low".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

/// Every `text` part across every `contents` entry, which is what the model
/// would actually read as conversation.
fn contents_texts(body: &serde_json::Value) -> Vec<String> {
    body["request"]["contents"]
        .as_array()
        .expect("contents array")
        .iter()
        .flat_map(|turn| turn["parts"].as_array().cloned().unwrap_or_default())
        .filter_map(|part| part["text"].as_str().map(str::to_string))
        .collect()
}

// Verifies: gh #280 - the system prompt is serialized *once*, in
// `systemInstruction`, and never appears as a `contents` turn. Before the fix
// the role match's `_ => "user"` arm caught `MessageRole::System`, so every
// request carried the whole preamble twice: once as instructions, once as a
// prior user turn.
#[test]
fn the_system_prompt_appears_once_and_never_in_contents() {
    let request = system_then_user("You are LCA. Be terse.", "list the files");
    let body = build_request(
        &request,
        "You are LCA. Be terse.",
        "p",
        "gemini-3.8-flash-low",
    );

    // Once, in systemInstruction.
    let system_parts = body["request"]["systemInstruction"]["parts"]
        .as_array()
        .expect("system parts");
    let system_texts: Vec<&str> = system_parts
        .iter()
        .filter_map(|part| part["text"].as_str())
        .collect();
    assert_eq!(
        system_texts,
        vec!["You are LCA. Be terse."],
        "instructions once"
    );

    // And not in contents.
    let texts = contents_texts(&body);
    assert_eq!(
        texts,
        vec!["list the files"],
        "contents holds only the turn"
    );
    assert!(
        !texts.iter().any(|text| text.contains("Be terse")),
        "the preamble is not a user turn: {texts:?}"
    );

    // The conversation opens on the genuine first user turn.
    assert_eq!(
        body["request"]["contents"][0]["role"].as_str(),
        Some("user"),
        "first contents entry"
    );
    assert_eq!(
        body["request"]["contents"][0]["parts"][0]["text"].as_str(),
        Some("list the files"),
        "and it is the user's actual turn"
    );
}

// Verifies: gh #280 - the cache consequence. The serialized prefix is stable
// across turns because nothing from the instructions sits in front of the
// conversation: the same user turn at the same depth yields the same leading
// `contents` entry whether or not a system message precedes it in the list.
#[test]
fn the_contents_prefix_is_the_same_with_or_without_a_system_message() {
    let body = |with_system: bool| {
        let messages = if with_system {
            vec![
                text_message(MessageRole::System, "instructions"),
                text_message(MessageRole::User, "first"),
                text_message(MessageRole::Assistant, "answer"),
            ]
        } else {
            vec![
                text_message(MessageRole::User, "first"),
                text_message(MessageRole::Assistant, "answer"),
            ]
        };
        let request = CompletionRequest {
            messages,
            tools: Vec::new(),
            model: "gemini-3.8-flash-low".to_string(),
            stable_prefix: 0,
            extras: Default::default(),
        };
        build_request(&request, "instructions", "p", "gemini-3.8-flash-low")
    };
    assert_eq!(
        body(true)["request"]["contents"],
        body(false)["request"]["contents"],
        "a system message adds nothing to the conversation prefix"
    );
}

// Verifies: gh #280 decision 2 - no silent loss. When more than one System
// message reaches the request, every one of them reaches the model. The
// driver passes the joined prompt (the blocks already collected), so the
// serialized `systemInstruction` carries that join once and does not repeat a
// block it already contains - while a block the join does *not* contain is
// appended rather than dropped (the row below).
#[test]
fn every_system_message_merges_into_system_instruction_parts() {
    let request = CompletionRequest {
        messages: vec![
            text_message(MessageRole::System, "first instruction"),
            text_message(MessageRole::System, "second instruction"),
            text_message(MessageRole::User, "go"),
        ],
        tools: Vec::new(),
        model: "gemini-3.8-flash-low".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    // The driver joins the System messages for `system`; pass that joined
    // string, as `StreamDriver::open` does.
    let body = build_request(
        &request,
        "first instruction\nsecond instruction",
        "p",
        "gemini-3.8-flash-low",
    );
    let system_texts: Vec<&str> = body["request"]["systemInstruction"]["parts"]
        .as_array()
        .expect("parts")
        .iter()
        .filter_map(|part| part["text"].as_str())
        .collect();
    assert_eq!(
        system_texts,
        vec!["first instruction\nsecond instruction"],
        "both instructions survive, once each, in systemInstruction"
    );
    let texts = contents_texts(&body);
    assert_eq!(texts, vec!["go"], "neither instruction is a contents turn");
}

// Verifies: gh #280 decision 2, the other half - a System message the caller
// did *not* fold into `system` is still not dropped. `build_request` is
// callable directly (tests, and any future caller), so the skip cannot assume
// the driver already collected them.
#[test]
fn a_system_message_absent_from_the_joined_prompt_is_still_carried() {
    let request = CompletionRequest {
        messages: vec![
            text_message(MessageRole::System, "collected"),
            text_message(MessageRole::System, "left behind"),
            text_message(MessageRole::User, "go"),
        ],
        tools: Vec::new(),
        model: "gemini-3.8-flash-low".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    // Only the first is passed as `system`, as a caller that pre-filtered by
    // some other rule might do.
    let body = build_request(&request, "collected", "p", "gemini-3.8-flash-low");
    let system_texts: Vec<&str> = body["request"]["systemInstruction"]["parts"]
        .as_array()
        .expect("parts")
        .iter()
        .filter_map(|part| part["text"].as_str())
        .collect();
    assert!(
        system_texts.contains(&"left behind"),
        "no silent loss: {system_texts:?}"
    );
    let texts = contents_texts(&body);
    assert_eq!(texts, vec!["go"], "still not a contents turn");
}

// Verifies: gh #280 - a conversation with no system message at all is
// unchanged. The fix must not shift an ordinary first user turn.
#[test]
fn a_request_with_no_system_message_is_unchanged() {
    let request = CompletionRequest {
        messages: vec![
            text_message(MessageRole::User, "first"),
            text_message(MessageRole::Assistant, "answer"),
        ],
        tools: Vec::new(),
        model: "gemini-3.8-flash-low".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let body = build_request(&request, "", "p", "gemini-3.8-flash-low");
    assert_eq!(
        body["request"]["contents"],
        serde_json::json!([
            { "role": "user", "parts": [{ "text": "first" }] },
            { "role": "model", "parts": [{ "text": "answer" }] },
        ])
    );
    // An empty prompt still sends one empty system part, the pre-existing
    // shape (pi sends its own default instruction block here).
    assert_eq!(
        body["request"]["systemInstruction"]["parts"][0]["text"].as_str(),
        Some("")
    );
}

// Verifies: gh #280 decision 4 - the tool-turn shape is untouched. The
// signature still rides as a *sibling* of `functionCall` (nesting is rejected
// by this gateway), and the result still arrives as a `user` turn carrying
// `functionResponse`.
#[test]
fn the_tool_turn_shape_is_unchanged_by_the_system_skip() {
    let assistant = lca_protocol::ChatMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::Reasoning {
            reasoning: "thinking".to_string(),
            signature: Some("sig-live".to_string()),
        }],
        tool_calls: vec![lca_protocol::ToolCall {
            call_id: "call-1".to_string(),
            name: "default_api:read".to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        }],
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    };
    let mut tool_result = text_message(MessageRole::Tool, "file body");
    tool_result.tool_call_id = Some("call-1".to_string());
    let request = CompletionRequest {
        messages: vec![
            text_message(MessageRole::System, "instructions"),
            text_message(MessageRole::User, "read it"),
            assistant,
            tool_result,
        ],
        tools: Vec::new(),
        model: "gemini-3.8-flash-low".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    let body = build_request(&request, "instructions", "p", "gemini-3.8-flash-low");
    let contents = body["request"]["contents"].as_array().expect("contents");
    assert_eq!(contents.len(), 3, "user, model, tool result: {contents:#?}");

    let call_part = contents[1]["parts"]
        .as_array()
        .expect("parts")
        .iter()
        .find(|part| part.get("functionCall").is_some())
        .expect("functionCall part");
    assert_eq!(call_part["thoughtSignature"].as_str(), Some("sig-live"));
    assert!(
        call_part["functionCall"].get("thoughtSignature").is_none(),
        "the signature is a sibling, never nested"
    );
    assert_eq!(
        contents[2]["role"].as_str(),
        Some("user"),
        "a tool result is a user turn"
    );
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"]["name"].as_str(),
        Some("default_api:read")
    );
}
