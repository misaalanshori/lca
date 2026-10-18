//! Gh #337: per-chunk `String::from_utf8_lossy` corrupted multi-byte UTF-8
//! codepoints split across TCP chunks.
//!
//! Both decoders on `lca-wire-openai` did
//! `buffer.push_str(&String::from_utf8_lossy(chunk))` per network chunk.
//! HTTP byte streams do not align with character boundaries, so a codepoint
//! straddling two chunks became `U+FFFD U+FFFD` — permanently corrupted text
//! no later stage could recover. A chat reply containing an emoji, an
//! accented letter, or CJK text corrupts whenever a packet boundary happens
//! to land mid-character, which is a matter of timing, not input.
//!
//! Frames now split at the byte level and only a *complete* frame is decoded,
//! so a split codepoint sits harmlessly in the buffer until its frame ends.
//!
//! Verifies: gh #337 acceptance 1-3 at the wire-contract level — every 2-,
//! 3-, and 4-byte codepoint at every split position, on both decoders.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::StreamEvent;
use lca_wire_openai::SseDecoder;

/// Decode `chunks` through the Chat Completions decoder, returning the text.
fn chat_text(chunks: &[&[u8]]) -> String {
    let mut decoder = SseDecoder::default();
    let mut events = Vec::new();
    for chunk in chunks {
        decoder.feed(chunk, &mut |event| events.push(event));
    }
    decoder.finish(&mut |event| events.push(event));
    events
        .into_iter()
        .filter_map(|event| match event {
            StreamEvent::TextDelta { delta } => Some(delta),
            _ => None,
        })
        .collect()
}

/// The same body through the Responses framing path.
fn responses_text(chunks: &[&[u8]]) -> String {
    let mut buffer = lca_wire_openai::sse::FrameBuffer::new();
    let mut frames = Vec::new();
    for chunk in chunks {
        buffer.feed(chunk, &mut |frame| frames.push(frame.to_string()));
    }
    if let Some(tail) = buffer.take_tail() {
        frames.push(tail);
    }
    let mut text = String::new();
    for frame in frames {
        for payload in lca_wire_openai::sse::frame_payloads(&frame) {
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue;
            };
            if let Some(delta) = value.get("delta").and_then(|d| d.as_str()) {
                text.push_str(delta);
            }
        }
    }
    text
}

fn chat_body(text: &str) -> String {
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({"choices": [{"delta": {"content": text}}]})
    )
}

fn responses_body(text: &str) -> String {
    format!(
        "data: {}\n\n",
        serde_json::json!({"type": "response.output_text.delta", "delta": text})
    )
}

// Verifies: gh #337 acceptance 1 — the whole straddle matrix survives on the
// Chat Completions decoder. On the old code these produce `U+FFFD`.
#[test]
fn the_full_straddle_matrix_survives_on_chat_completions() {
    for sample in ["é", "中", "😀"] {
        let body = chat_body(sample);
        let bytes = body.as_bytes();
        let start = body.find(sample).expect("in body");
        for split in 1..sample.len() {
            let at = start + split;
            let text = chat_text(&[&bytes[..at], &bytes[at..]]);
            assert_eq!(
                text, sample,
                "`{sample}` split after byte {split} decoded as {text:?}"
            );
        }
    }
}

// Verifies: gh #337 — the same matrix on the Responses path, so the twin
// cannot drift again.
#[test]
fn the_full_straddle_matrix_survives_on_responses() {
    for sample in ["é", "中", "😀"] {
        let body = responses_body(sample);
        let bytes = body.as_bytes();
        let start = body.find(sample).expect("in body");
        for split in 1..sample.len() {
            let at = start + split;
            let text = responses_text(&[&bytes[..at], &bytes[at..]]);
            assert_eq!(
                text, sample,
                "`{sample}` split after byte {split} decoded as {text:?}"
            );
        }
    }
}

// Verifies: gh #337 — the replacement character never appears. This is the
// direct assertion against the defect: a per-chunk lossy decode is exactly
// what emits `U+FFFD`.
#[test]
fn no_split_ever_yields_a_replacement_character() {
    for sample in ["é", "中", "😀"] {
        let body = chat_body(sample);
        let bytes = body.as_bytes();
        let start = body.find(sample).expect("in body");
        for split in 1..sample.len() {
            let at = start + split;
            let text = chat_text(&[&bytes[..at], &bytes[at..]]);
            assert!(
                !text.contains('\u{FFFD}'),
                "`{sample}` split after byte {split} produced U+FFFD: {text:?}"
            );
        }
    }
}

// Verifies: gh #337 acceptance 2 — framing still dispatches: a body of two
// deltas split mid-codepoint still yields both, in order.
#[test]
fn framing_still_dispatches_every_event_across_a_split() {
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({"choices": [{"delta": {"content": "aé"}}]}),
        serde_json::json!({"choices": [{"delta": {"content": "中😀"}}]}),
    );
    let bytes = body.as_bytes();
    // Split inside the 3-byte CJK codepoint of the second frame.
    let at = body.find('中').expect("in body") + 1;
    let text = chat_text(&[&bytes[..at], &bytes[at..]]);
    assert_eq!(text, "aé中😀");
}

// Verifies: gh #337 acceptance 3 — the pathological case: a body delivered
// one byte at a time decodes identically to one chunk. Framing must not
// depend on chunk alignment at all.
#[test]
fn byte_at_a_time_delivery_decodes_identically() {
    let expected = "héllo 中文 😀";
    let body = chat_body(expected);
    let bytes = body.as_bytes();
    let chunks: Vec<&[u8]> = bytes.chunks(1).collect();
    assert_eq!(chat_text(&chunks), expected);
    assert_eq!(chat_text(&[bytes]), expected);
}
