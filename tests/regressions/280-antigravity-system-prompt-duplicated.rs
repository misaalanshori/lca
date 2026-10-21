//! GitHub #280: the antigravity request sent the system prompt twice.
//!
//! `build_request` extracted every `MessageRole::System` message into
//! `systemInstruction` (correct) and then serialized the same messages into
//! `contents` again, because the role match ended in a wildcard:
//!
//! ```ignore
//! let role = match message.role {
//!     MessageRole::Tool => "user",
//!     MessageRole::Assistant => "model",
//!     _ => "user",          // <- caught MessageRole::System
//! };
//! ```
//!
//! So every request carried the whole preamble twice — once as instructions,
//! once as a prior user turn. Three consequences, in order of how much they
//! cost:
//!
//! 1. **Input tokens.** The preamble is the largest fixed block in the request
//!    and it was billed twice on every turn.
//! 2. **Framing.** Directives arrived as something the user had said, which is
//!    both a fidelity problem and a prompt-injection surface: text shaped like
//!    a user turn is text the model may treat as a request.
//! 3. **Cache.** The leading `contents` entry was the preamble, so the
//!    conversation prefix was one turn deeper than it should be and shifted
//!    whenever anything upstream touched the instructions. Same class as
//!    gh #299 (a `System` message treated as conversational content).
//!
//! The fix is the arm the issue proposes and pi-antigravity already ships
//! (`src/stream/stream.ts:317`, `if ((msg.role as string) === "system")
//! continue;`): a `System` message never becomes a `contents` turn. The arms
//! are exhaustive over `MessageRole` so the next variant cannot fall into
//! `"user"` the same way.
//!
//! Verifies: NFR-24 (a shipped defect's guard), GitHub issue #280 acceptance
//! 1-3.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use antigravity::build_request;
use lca_protocol::{ChatMessage, CompletionRequest, ContentBlock, MessageRole};

/// A text-only message in the given role.
fn text_message(role: MessageRole, text: &str) -> ChatMessage {
    ChatMessage {
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

/// A request with exactly the given messages.
fn request(messages: Vec<ChatMessage>) -> CompletionRequest {
    CompletionRequest {
        messages,
        tools: Vec::new(),
        model: "gemini-3.8-flash-low".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

/// Every `text` part across every `contents` entry — what the model reads as
/// conversation.
fn contents_texts(body: &serde_json::Value) -> Vec<String> {
    body["request"]["contents"]
        .as_array()
        .expect("contents array")
        .iter()
        .flat_map(|turn| turn["parts"].as_array().cloned().unwrap_or_default())
        .filter_map(|part| part["text"].as_str().map(str::to_string))
        .collect()
}

/// Every `text` part in `systemInstruction`.
fn system_texts(body: &serde_json::Value) -> Vec<String> {
    body["request"]["systemInstruction"]["parts"]
        .as_array()
        .expect("system parts")
        .iter()
        .filter_map(|part| part["text"].as_str().map(str::to_string))
        .collect()
}

/// The preamble from the issue, long enough that a duplicate is obvious.
const PREAMBLE: &str = "You are LCA, a terminal coding agent.\n\
                        Follow the repository's AGENTS.md.\n\
                        Prefer the smallest change that fully works.";

// ---------------------------------------------------------------------------
// Acceptance 1: the system prompt is exclusively in `systemInstruction`
// ---------------------------------------------------------------------------

/// **The issue's row.** One system prompt plus one user turn: the preamble
/// appears once, in `systemInstruction`, and the serialized `contents` array
/// holds only the user's turn.
#[test]
fn the_preamble_is_serialized_once_and_never_as_a_contents_turn() {
    let body = build_request(
        &request(vec![
            text_message(MessageRole::System, PREAMBLE),
            text_message(MessageRole::User, "what does this repo do?"),
        ]),
        PREAMBLE,
        "proj",
        "gemini-3.8-flash-low",
    );

    assert_eq!(
        system_texts(&body),
        vec![PREAMBLE],
        "the instructions ride once, in systemInstruction"
    );
    assert_eq!(
        contents_texts(&body),
        vec!["what does this repo do?"],
        "contents holds the turn and nothing else"
    );
}

// ---------------------------------------------------------------------------
// Acceptance 2: the conversation starts with the genuine first user turn
// ---------------------------------------------------------------------------

/// The first `contents` entry is the user's actual turn, with the user role —
/// not a preamble wearing the user role.
#[test]
fn contents_opens_on_the_genuine_first_user_turn() {
    let body = build_request(
        &request(vec![
            text_message(MessageRole::System, PREAMBLE),
            text_message(MessageRole::User, "first"),
            text_message(MessageRole::Assistant, "answer"),
            text_message(MessageRole::User, "second"),
        ]),
        PREAMBLE,
        "proj",
        "gemini-3.8-flash-low",
    );
    let contents = body["request"]["contents"].as_array().expect("contents");
    assert_eq!(contents.len(), 3, "three turns, no preamble turn");
    assert_eq!(contents[0]["role"].as_str(), Some("user"));
    assert_eq!(contents[0]["parts"][0]["text"].as_str(), Some("first"));
    assert_eq!(contents[1]["role"].as_str(), Some("model"));
    assert_eq!(contents[2]["role"].as_str(), Some("user"));
}

/// The preamble is not merely absent from the first position — it appears
/// nowhere in `contents`, at any depth. A fix that skipped only index 0 would
/// pass the row above and fail this one.
#[test]
fn no_contents_entry_carries_the_preamble_text() {
    let body = build_request(
        &request(vec![
            text_message(MessageRole::System, PREAMBLE),
            text_message(MessageRole::User, "first"),
            // A transform that re-injects the prompt mid-conversation.
            text_message(MessageRole::System, PREAMBLE),
            text_message(MessageRole::Assistant, "answer"),
        ]),
        PREAMBLE,
        "proj",
        "gemini-3.8-flash-low",
    );
    let serialized = serde_json::to_string(&body["request"]["contents"]).expect("serializes");
    assert!(
        !serialized.contains("terminal coding agent"),
        "no preamble text anywhere in contents: {serialized}"
    );
    assert_eq!(contents_texts(&body), vec!["first", "answer"]);
}

// ---------------------------------------------------------------------------
// The cache consequence (decision 3)
// ---------------------------------------------------------------------------

/// The serialized conversation prefix is identical with and without a system
/// message, which is what makes it cacheable: the instructions change nothing
/// about where the conversation starts or what it contains.
#[test]
fn the_serialized_conversation_prefix_ignores_the_preamble() {
    let with = build_request(
        &request(vec![
            text_message(MessageRole::System, PREAMBLE),
            text_message(MessageRole::User, "first"),
            text_message(MessageRole::Assistant, "answer"),
        ]),
        PREAMBLE,
        "proj",
        "gemini-3.8-flash-low",
    );
    let without = build_request(
        &request(vec![
            text_message(MessageRole::User, "first"),
            text_message(MessageRole::Assistant, "answer"),
        ]),
        PREAMBLE,
        "proj",
        "gemini-3.8-flash-low",
    );
    assert_eq!(
        with["request"]["contents"], without["request"]["contents"],
        "the preamble does not occupy a turn, so the prefix is stable"
    );
    assert_eq!(
        with["request"]["systemInstruction"], without["request"]["systemInstruction"],
        "and the instructions themselves do not vary with message order"
    );
}

// ---------------------------------------------------------------------------
// Decision 2: no silent loss
// ---------------------------------------------------------------------------

/// Two System messages, both folded into the joined prompt: the join is sent
/// once, and neither block is repeated as a second part.
#[test]
fn a_joined_multi_system_prompt_is_sent_once() {
    let body = build_request(
        &request(vec![
            text_message(MessageRole::System, "rule one"),
            text_message(MessageRole::System, "rule two"),
            text_message(MessageRole::User, "go"),
        ]),
        "rule one\nrule two",
        "proj",
        "gemini-3.8-flash-low",
    );
    assert_eq!(
        system_texts(&body),
        vec!["rule one\nrule two"],
        "the join once, not the join plus its parts"
    );
    assert_eq!(contents_texts(&body), vec!["go"]);
}

/// A System message the caller did *not* fold into `system` still reaches the
/// model, appended to the parts array. This is the row that makes the fix a
/// merge rather than a drop: pi-antigravity's `systemParts` is an array
/// precisely so the prompt plus injected blocks all travel
/// (`src/stream/stream.ts:891-897`).
#[test]
fn a_system_message_missing_from_the_joined_prompt_is_appended_not_dropped() {
    let body = build_request(
        &request(vec![
            text_message(MessageRole::System, "collected"),
            text_message(MessageRole::System, "a skill block nobody folded in"),
            text_message(MessageRole::User, "go"),
        ]),
        "collected",
        "proj",
        "gemini-3.8-flash-low",
    );
    let texts = system_texts(&body);
    assert_eq!(
        texts,
        vec!["collected", "a skill block nobody folded in"],
        "the uncollected block survives in the parts array: {texts:?}"
    );
    assert_eq!(contents_texts(&body), vec!["go"], "still not a turn");
}

/// `parts` is never empty. An empty array would be a different message on this
/// gateway, and the pre-fix code always sent one part; the guard keeps that.
#[test]
fn system_instruction_always_carries_at_least_one_part() {
    for (messages, system) in [
        (vec![text_message(MessageRole::User, "hi")], ""),
        (vec![text_message(MessageRole::User, "hi")], "sys"),
        (
            vec![
                text_message(MessageRole::System, "sys"),
                text_message(MessageRole::User, "hi"),
            ],
            "sys",
        ),
    ] {
        let body = build_request(&request(messages), system, "proj", "gemini-3.8-flash-low");
        let parts = body["request"]["systemInstruction"]["parts"]
            .as_array()
            .expect("parts");
        assert!(!parts.is_empty(), "never an empty parts array");
    }
}

// ---------------------------------------------------------------------------
// Decision 4: nothing else moved
// ---------------------------------------------------------------------------

/// The thoughtSignature still rides as a **sibling** of `functionCall`.
/// Nesting is the shape this gateway rejects as an unknown field, and the
/// system fix must not have disturbed it.
#[test]
fn a_tool_turn_still_carries_the_signature_as_a_sibling() {
    let mut assistant = text_message(MessageRole::Assistant, "let me look");
    assistant.content.push(ContentBlock::Reasoning {
        reasoning: "thinking".to_string(),
        signature: Some("sig-abc".to_string()),
    });
    assistant.tool_calls = vec![lca_protocol::ToolCall {
        call_id: "call-9".to_string(),
        name: "default_api:read".to_string(),
        arguments: "{}".to_string(),
        parent_call_id: None,
    }];
    let mut tool_result = text_message(MessageRole::Tool, "file body");
    tool_result.tool_call_id = Some("call-9".to_string());

    let body = build_request(
        &request(vec![
            text_message(MessageRole::System, PREAMBLE),
            text_message(MessageRole::User, "read it"),
            assistant,
            tool_result,
        ]),
        PREAMBLE,
        "proj",
        "gemini-3.8-flash-low",
    );
    let contents = body["request"]["contents"].as_array().expect("contents");
    assert_eq!(contents.len(), 3, "user, model, tool result");
    let call_part = contents[1]["parts"]
        .as_array()
        .expect("parts")
        .iter()
        .find(|part| part.get("functionCall").is_some())
        .expect("functionCall part");
    assert_eq!(
        call_part["thoughtSignature"].as_str(),
        Some("sig-abc"),
        "the signature is a sibling of functionCall"
    );
    assert!(
        call_part["functionCall"].get("thoughtSignature").is_none(),
        "never nested inside functionCall"
    );
    assert_eq!(contents[2]["role"].as_str(), Some("user"));
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"]["name"].as_str(),
        Some("default_api:read")
    );
}

/// A conversation with no System message serializes exactly as before.
#[test]
fn a_conversation_without_a_system_message_is_unchanged() {
    let body = build_request(
        &request(vec![
            text_message(MessageRole::User, "first"),
            text_message(MessageRole::Assistant, "answer"),
        ]),
        "",
        "proj",
        "gemini-3.8-flash-low",
    );
    assert_eq!(
        body["request"]["contents"],
        serde_json::json!([
            { "role": "user", "parts": [{ "text": "first" }] },
            { "role": "model", "parts": [{ "text": "answer" }] },
        ])
    );
}

/// The envelope's step label is derived from `contents.len()`, so the fix
/// changes it by exactly one when a System message is present — the preamble no
/// longer counts as a content block. Pinned so the label cannot drift silently.
#[test]
fn the_step_label_counts_turns_not_instructions() {
    let body = |with_system: bool| {
        let mut messages = Vec::new();
        if with_system {
            messages.push(text_message(MessageRole::System, PREAMBLE));
        }
        messages.push(text_message(MessageRole::User, "first"));
        messages.push(text_message(MessageRole::Assistant, "answer"));
        let request = request(messages);
        build_request(&request, PREAMBLE, "proj", "gemini-3.8-flash-low")
    };
    let with = body(true);
    let without = body(false);
    assert_eq!(
        with["request"]["labels"]["last_step_index"],
        without["request"]["labels"]["last_step_index"],
        "one content block per turn, instructions excluded"
    );
    assert_eq!(
        with["request"]["labels"]["last_step_index"].as_str(),
        Some("1"),
        "two turns, 0-based last index 1"
    );
}
