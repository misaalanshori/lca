//! Gh #337: per-chunk `String::from_utf8_lossy` corrupted multi-byte UTF-8
//! codepoints split across TCP chunks.
//!
//! HTTP byte streams do not align with character boundaries. Both decoders
//! used to do `buffer.push_str(&String::from_utf8_lossy(chunk))` per chunk,
//! so a codepoint straddling two chunks became `U+FFFD U+FFFD` — permanently
//! corrupted text that no later stage could recover.
//!
//! The stub covers the full straddle matrix: every 2-, 3-, and 4-byte
//! codepoint split at every possible position within the codepoint, fed as
//! two chunks, through the real decoders. Nothing here asserts a byte count;
//! the assertion is always the decoded text an end user would see.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::StreamEvent;
use lca_wire_openai::SseDecoder;

/// The decoders under test: one function type, so the matrix runs against
/// both without duplicating it.
type Decoder = fn(&[&[u8]]) -> (String, Vec<String>);

/// Drive the Chat Completions decoder with `chunks`, returning (text, errors).
fn chat_completions_decode(chunks: &[&[u8]]) -> (String, Vec<String>) {
    let mut decoder = SseDecoder::default();
    let mut events = Vec::new();
    for chunk in chunks {
        decoder.feed(chunk, &mut |event| events.push(event));
    }
    decoder.finish(&mut |event| events.push(event));
    collect(events)
}

/// Drive the Responses driver's framing path. The driver owns a socket, so
/// its pure framing half is exercised through the shared `FrameBuffer` it
/// uses — the same bytewise split, the same complete-frame decode. The
/// driver-level rows live in `tests/responses.rs`.
fn responses_decode(chunks: &[&[u8]]) -> (String, Vec<String>) {
    let mut buffer = lca_wire_openai::sse::FrameBuffer::new();
    let mut frames = Vec::new();
    for chunk in chunks {
        buffer.feed(chunk, &mut |frame| frames.push(frame.to_string()));
    }
    if let Some(tail) = buffer.take_tail() {
        frames.push(tail);
    }
    let mut text = String::new();
    let mut errors = Vec::new();
    for frame in frames {
        for payload in lca_wire_openai::sse::frame_payloads(&frame) {
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue;
            };
            // The Responses text-delta event shape.
            if value.get("type").and_then(|t| t.as_str()) == Some("response.output_text.delta")
                && let Some(delta) = value.get("delta").and_then(|d| d.as_str())
            {
                text.push_str(delta);
            }
            if value.get("type").and_then(|t| t.as_str()) == Some("error")
                && let Some(message) = value
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
            {
                errors.push(message.to_string());
            }
        }
    }
    (text, errors)
}

fn collect(events: Vec<StreamEvent>) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut errors = Vec::new();
    for event in events {
        match event {
            StreamEvent::TextDelta { delta } => text.push_str(&delta),
            StreamEvent::Error { message, .. } => errors.push(message),
            _ => {}
        }
    }
    (text, errors)
}

/// The 2-, 3-, and 4-byte codepoints the matrix uses: one per byte length,
/// each a real character a user would type.
const SAMPLES: &[&str] = &[
    "é",  // U+00E9, 2 bytes
    "中", // U+4E2D, 3 bytes
    "😀", // U+1F600, 4 bytes
];

/// Build the SSE body carrying `text` as one text delta.
fn chat_body(text: &str) -> String {
    let payload = serde_json::json!({
        "choices": [{"delta": {"content": text}}],
    });
    format!("data: {payload}\n\ndata: [DONE]\n\n")
}

/// The same for the Responses shape.
fn responses_body(text: &str) -> String {
    let payload = serde_json::json!({
        "type": "response.output_text.delta",
        "delta": text,
    });
    format!("data: {payload}\n\n")
}

// Verifies: gh #337 acceptance 1+3 - every multi-byte codepoint survives a
// split at every byte position, on the Chat Completions decoder. Before the
// fix each of these produced `U+FFFD` pairs.
#[test]
fn every_split_position_of_every_codepoint_survives_chat_completions() {
    for sample in SAMPLES {
        let body = chat_body(sample);
        let bytes = body.as_bytes();
        // The codepoint's byte offset inside the serialized frame.
        let start = body.find(sample).expect("the sample is in the body");
        let width = sample.len();
        for split in 1..width {
            let at = start + split;
            let (text, errors) = chat_completions_decode(&[&bytes[..at], &bytes[at..]]);
            assert_eq!(
                text, *sample,
                "`{sample}` (width {width}) split after byte {split} decoded as {text:?}"
            );
            assert!(
                errors.is_empty(),
                "`{sample}` split after byte {split} produced errors: {errors:?}"
            );
        }
    }
}

// Verifies: gh #337 - the same matrix on the Responses framing path.
#[test]
fn every_split_position_of_every_codepoint_survives_responses() {
    for sample in SAMPLES {
        let body = responses_body(sample);
        let bytes = body.as_bytes();
        let start = body.find(sample).expect("in body");
        let width = sample.len();
        for split in 1..width {
            let at = start + split;
            let (text, errors) = responses_decode(&[&bytes[..at], &bytes[at..]]);
            assert_eq!(
                text, *sample,
                "`{sample}` split after byte {split} decoded as {text:?}"
            );
            assert!(errors.is_empty(), "errors: {errors:?}");
        }
    }
}

// Verifies: gh #337 - the codepoint split across a FRAME boundary, not merely
// mid-frame, still assembles: the frame terminator is found bytewise, so the
// partial bytes sit in the buffer until the frame completes.
#[test]
fn a_codepoint_split_across_the_chunk_holding_the_terminator_survives() {
    let sample = "😀";
    let body = chat_body(sample);
    let bytes = body.as_bytes();
    let start = body.find(sample).expect("in body");
    // Split one byte before the codepoint, so the chunk boundary is right
    // where the JSON payload's text begins.
    let at = start - 1;
    let (text, _) = chat_completions_decode(&[&bytes[..at], &bytes[at..]]);
    assert_eq!(text, sample);
}

// Verifies: gh #337 - a byte-at-a-time delivery of a whole body decodes
// identically to delivering it in one piece. This is the pathological TCP
// case (1000-byte MSS with tiny segments) and the strongest form of the
// property: framing must not depend on chunk alignment at all.
#[test]
fn a_body_delivered_one_byte_at_a_time_decodes_identically() {
    let text = "héllo 中文 😀 done";
    let body = chat_body(text);
    let bytes = body.as_bytes();
    let chunks: Vec<&[u8]> = bytes.chunks(1).collect();
    let (one_byte, errors) = chat_completions_decode(&chunks);
    assert_eq!(one_byte, text, "byte-at-a-time text");
    assert!(errors.is_empty(), "{errors:?}");

    // And the same body in one chunk, for the equality the row's name claims.
    let (whole, _) = chat_completions_decode(&[bytes]);
    assert_eq!(whole, text);
}

// Verifies: gh #337 - the replacement character never appears in decoded
// text for any split. This is the direct assertion against the defect: a
// lossy per-chunk decode is exactly what produces U+FFFD, so its absence in
// the output for every split position is the contract.
#[test]
fn no_replacement_character_appears_for_any_split() {
    for sample in SAMPLES {
        let body = chat_body(sample);
        let bytes = body.as_bytes();
        let start = body.find(sample).expect("in body");
        for split in 1..sample.len() {
            let at = start + split;
            let (text, _) = chat_completions_decode(&[&bytes[..at], &bytes[at..]]);
            assert!(
                !text.contains('\u{FFFD}'),
                "`{sample}` split after byte {split} produced U+FFFD: {text:?}"
            );
        }
    }
}

// Verifies: gh #337 - two decoders, one framing rule. Running the same split
// through both yields the same text, so the twin cannot silently drift again.
#[test]
fn both_decoders_agree_on_every_split() {
    for sample in SAMPLES {
        let chat = chat_body(sample);
        let responses = responses_body(sample);
        for split in 1..sample.len() {
            let chat_at = chat.find(sample).expect("in body") + split;
            let responses_at = responses.find(sample).expect("in body") + split;
            let (from_chat, _) = chat_completions_decode(&[
                &chat.as_bytes()[..chat_at],
                &chat.as_bytes()[chat_at..],
            ]);
            let (from_responses, _) = responses_decode(&[
                &responses.as_bytes()[..responses_at],
                &responses.as_bytes()[responses_at..],
            ]);
            assert_eq!(
                from_chat, from_responses,
                "`{sample}` split after byte {split} disagreed between decoders"
            );
        }
    }
}

/// Keep the `Decoder` alias honest: both functions must have its shape.
#[allow(dead_code)]
fn _both_are_decoders() -> [Decoder; 2] {
    [chat_completions_decode, responses_decode]
}
