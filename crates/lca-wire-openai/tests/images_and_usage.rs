//! Gh #338: `build_responses_body` dropped user image content and
//! `responses_usage` hardcoded `cache_read: 0`.
//!
//! Both defects are visible in the request/response JSON this kit speaks, so
//! these rows assert the wire shapes directly:
//!
//! - a user image serializes into the input message's content array as an
//!   `input_image` part (pi's `openai-responses-shared` shape), and an
//!   image-only message still produces an input item;
//! - `usage.input_tokens_details.cached_tokens` maps to `cache_read`, with
//!   `input` carrying the uncached remainder (the same subtraction the Chat
//!   Completions mapper does).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::{ChatMessage, CompletionRequest, ContentBlock, MessageRole};
use lca_wire_openai::responses::{build_responses_body, responses_usage};

fn user(blocks: Vec<ContentBlock>) -> ChatMessage {
    ChatMessage {
        role: MessageRole::User,
        content: blocks,
        tool_calls: Vec::new(),
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    }
}

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text {
        text: text.to_string(),
    }
}

fn image(media_type: &str, bytes: &[u8]) -> ContentBlock {
    ContentBlock::Image {
        media_type: media_type.to_string(),
        bytes: bytes.to_vec(),
    }
}

fn request(messages: Vec<ChatMessage>) -> CompletionRequest {
    CompletionRequest {
        messages,
        tools: Vec::new(),
        model: "m".to_string(),
        stable_prefix: 0,
        extras: Default::default(),
    }
}

// Verifies: gh #338 acceptance 1 - a user image rides the request in the
// `input_image` shape, as a data URL carrying the exact bytes.
#[test]
fn a_user_image_serializes_as_an_input_image_part() {
    // `AQID` is the base64 of [1, 2, 3].
    let body = build_responses_body(
        &request(vec![user(vec![
            text("what is this?"),
            image("image/png", &[1, 2, 3]),
        ])]),
        "sys",
        "m",
        None,
    );
    let content = body["input"][0]["content"]
        .as_array()
        .expect("the user message carries a content array");
    assert_eq!(content.len(), 2, "text and image both present: {content:?}");
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "what is this?");
    assert_eq!(content[1]["type"], "input_image");
    assert_eq!(
        content[1]["image_url"], "data:image/png;base64,AQID",
        "the bytes inline as a data URL"
    );
    assert_eq!(
        content[1]["detail"], "auto",
        "pi's shape carries `detail: auto`"
    );
}

// Verifies: gh #338 acceptance 1 - an IMAGE-ONLY user message still produces
// an input item. Before the fix the joined text was empty and the whole
// message vanished, so the model received nothing at all.
#[test]
fn an_image_only_user_message_still_produces_an_input_item() {
    let body = build_responses_body(
        &request(vec![user(vec![image("image/jpeg", &[9, 9])])]),
        "sys",
        "m",
        None,
    );
    let input = body["input"].as_array().expect("input is an array");
    assert_eq!(
        input.len(),
        1,
        "the image-only message is present: {input:?}"
    );
    let content = input[0]["content"]
        .as_array()
        .expect("it carries a content array");
    assert_eq!(content.len(), 1, "exactly the image: {content:?}");
    assert_eq!(content[0]["type"], "input_image");
    assert_eq!(content[0]["image_url"], "data:image/jpeg;base64,CQk=");
    assert_eq!(input[0]["role"], "user");
}

// Verifies: gh #338 - text and image ORDER follows the message's own blocks
// within each lane: text parts are joined first, then images, matching pi's
// assembly. Two images keep their relative order.
#[test]
fn multiple_images_keep_their_order_after_the_text() {
    let body = build_responses_body(
        &request(vec![user(vec![
            image("image/png", &[1]),
            text("between"),
            image("image/png", &[2]),
        ])]),
        "sys",
        "m",
        None,
    );
    let content = body["input"][0]["content"].as_array().expect("array");
    assert_eq!(content.len(), 3);
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "between");
    assert_eq!(content[1]["image_url"], "data:image/png;base64,AQ==");
    assert_eq!(content[2]["image_url"], "data:image/png;base64,Ag==");
}

// Verifies: gh #338 acceptance 1 + 4 - a text-only message keeps exactly the
// shape it had before (one `input_text` part), so this fix changed nothing
// for the common case.
#[test]
fn a_text_only_user_message_is_unchanged() {
    let body = build_responses_body(&request(vec![user(vec![text("plain")])]), "sys", "m", None);
    let content = body["input"][0]["content"].as_array().expect("array");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "plain");
}

// Verifies: gh #338 acceptance 2 - `cached_tokens` maps to `cache_read`, and
// `input` becomes the uncached remainder. This is what makes a cache hit
// stop being billed as fresh input on codex/grok.
#[test]
fn cached_tokens_map_to_cache_read() {
    let response = serde_json::json!({
        "usage": {
            "input_tokens": 1000,
            "input_tokens_details": {"cached_tokens": 800},
            "output_tokens": 50,
        },
    });
    let usage = responses_usage(&response).expect("usage parses");
    assert_eq!(usage.cache_read, 800, "the cache hit is credited");
    assert_eq!(usage.input, 200, "input is the uncached remainder");
    assert_eq!(usage.output, 50);
}

// Verifies: gh #338 - a response with no `input_tokens_details` still works
// (cache_read 0, input unchanged), so an endpoint that omits the detail does
// not change the accounting.
#[test]
fn a_missing_cache_detail_leaves_the_old_numbers() {
    let response = serde_json::json!({
        "usage": {"input_tokens": 40, "output_tokens": 7},
    });
    let usage = responses_usage(&response).expect("usage parses");
    assert_eq!(usage.cache_read, 0);
    assert_eq!(usage.input, 40, "nothing is subtracted");
}

// Verifies: gh #338 - the detail present but null/absent `cached_tokens` is
// the same as absent (the API omits it on a miss).
#[test]
fn a_null_cached_tokens_is_zero() {
    let response = serde_json::json!({
        "usage": {
            "input_tokens": 12,
            "input_tokens_details": {"cached_tokens": null},
            "output_tokens": 2,
        },
    });
    let usage = responses_usage(&response).expect("usage parses");
    assert_eq!(usage.cache_read, 0);
    assert_eq!(usage.input, 12);
}

// Verifies: gh #338 - a cache_read larger than input cannot underflow: the
// subtraction saturates (a hostile or buggy endpoint must not panic a turn).
#[test]
fn an_inconsistent_cache_read_saturates_instead_of_underflowing() {
    let response = serde_json::json!({
        "usage": {
            "input_tokens": 10,
            "input_tokens_details": {"cached_tokens": 9999},
            "output_tokens": 1,
        },
    });
    let usage = responses_usage(&response).expect("usage parses");
    assert_eq!(usage.cache_read, 9999, "the reported hit is preserved");
    assert_eq!(usage.input, 0, "the remainder saturates at zero");
}

// Verifies: gh #338 - a terminal response with a cache hit and no output
// still reports usage (the zero-check is on the API's own numbers, before
// the subtraction), so a cached prefill is not swallowed.
#[test]
fn a_cached_only_usage_still_reports() {
    let response = serde_json::json!({
        "usage": {
            "input_tokens": 500,
            "input_tokens_details": {"cached_tokens": 500},
            "output_tokens": 0,
        },
    });
    let usage = responses_usage(&response).expect("100% cache hit still reports");
    assert_eq!(usage.cache_read, 500);
    assert_eq!(usage.input, 0);
}
