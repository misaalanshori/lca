//! Gh #338: `build_responses_body` dropped user image content, and
//! `responses_usage` hardcoded `cache_read: 0`.
//!
//! `/v1/responses` is what `extensions/codex` and `extensions/grok` speak.
//! Two defects on that wire kit:
//!
//! 1. The user serializer filtered `ContentBlock::Text` only, so an attached
//!    image was discarded — and an image-only message joined to an empty
//!    string and vanished, meaning the model received no message at all.
//! 2. `responses_usage` read `input_tokens`/`output_tokens` and hardcoded
//!    `cache_read: 0`, so every prompt-cache hit on those providers was
//!    counted as fresh input: the footer and `/usage` overstated spend.
//!
//! Verifies: gh #338 acceptance 1-3 at the wire-contract JSON level.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::{ChatMessage, CompletionRequest, ContentBlock, MessageRole};
use lca_wire_openai::responses::{build_responses_body, responses_usage};

fn body_with(blocks: Vec<ContentBlock>) -> serde_json::Value {
    let request = CompletionRequest {
        messages: vec![ChatMessage {
            role: MessageRole::User,
            content: blocks,
            tool_calls: Vec::new(),
            tool_call_id: None,
            usage: None,
            extras: Default::default(),
        }],
        tools: Vec::new(),
        model: "m".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    };
    build_responses_body(&request, "sys", "m", None)
}

// Verifies: gh #338 acceptance 1 — a user image serializes as an
// `input_image` part with the bytes inlined as a data URL, in pi's shape
// (`detail: "auto"`), beside the text part.
#[test]
fn a_user_image_reaches_the_request_as_input_image() {
    let body = body_with(vec![
        ContentBlock::Text {
            text: "describe".to_string(),
        },
        ContentBlock::Image {
            media_type: "image/png".to_string(),
            bytes: vec![1, 2, 3],
        },
    ]);
    let content = body["input"][0]["content"].as_array().expect("array");
    assert_eq!(content.len(), 2, "{content:?}");
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[1]["type"], "input_image");
    assert_eq!(content[1]["image_url"], "data:image/png;base64,AQID");
    assert_eq!(content[1]["detail"], "auto");
}

// Verifies: gh #338 acceptance 1 — an image-only user message still produces
// an input item. On the old code the joined text was empty and the message
// was omitted entirely.
#[test]
fn an_image_only_message_is_not_dropped() {
    let body = body_with(vec![ContentBlock::Image {
        media_type: "image/jpeg".to_string(),
        bytes: vec![9, 9],
    }]);
    let input = body["input"].as_array().expect("array");
    assert_eq!(input.len(), 1, "the message survives: {input:?}");
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[0]["content"][0]["type"], "input_image");
    assert_eq!(
        input[0]["content"][0]["image_url"],
        "data:image/jpeg;base64,CQk="
    );
}

// Verifies: gh #338 acceptance 2 — `input_tokens_details.cached_tokens` maps
// to `cache_read`, with `input` carrying the uncached remainder.
#[test]
fn cached_tokens_become_cache_read() {
    let response = serde_json::json!({
        "usage": {
            "input_tokens": 1000,
            "input_tokens_details": {"cached_tokens": 800},
            "output_tokens": 50,
        },
    });
    let usage = responses_usage(&response).expect("parses");
    assert_eq!(usage.cache_read, 800);
    assert_eq!(usage.input, 200, "the uncached remainder");
    assert_eq!(usage.output, 50);
}

// Verifies: gh #338 acceptance 3 — a text-only message is byte-for-byte the
// shape it was before the change, so the common request is untouched.
#[test]
fn a_text_only_message_keeps_its_old_shape() {
    let body = body_with(vec![ContentBlock::Text {
        text: "hi".to_string(),
    }]);
    assert_eq!(
        body["input"][0]["content"].as_array().expect("array").len(),
        1
    );
    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(body["input"][0]["content"][0]["text"], "hi");
}
