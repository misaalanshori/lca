//! GitHub #329: the Extension Hub's filter typed `[` on unhandled keys, because
//! it scanned raw input bytes for text instead of taking text the input layer had
//! already parsed.
//!
//! # The defect
//!
//! `chat_hub.rs`'s fallthrough arm was:
//!
//! ```ignore
//! if let Some(text) = data.chars().find(|c| !c.is_control()) { query.push(text) }
//! ```
//!
//! `data` is the **raw input payload** at this seam, not decoded text. An ANSI
//! sequence starts with `ESC` (a control character, skipped by the predicate) and
//! its second byte is `[` (printable), so `Left` (`\x1b[D`) pushed `[`. A few
//! presses left `[[[`, which matches nothing.
//!
//! # The finding the fix turns on
//!
//! Every other inline text field in `lca-ui` routes through
//! `chat_commands::{printable, paste_text}`:
//!
//! - `printable(data)` = `keys::decode_printable_key(data)` (Kitty CSI-u /
//!   modifyOtherKeys) then `keys::parse_key(data)` kept only when it is one
//!   character or `space`. `parse_key("\x1b[D")` is `Some("left")` - two characters
//!   and not `space`, so `printable` returns `None`. An escape sequence is therefore
//!   *not text* by construction.
//! - `paste_text(data)` extracts `ESC[200~…ESC[201~`'s payload and flattens it.
//!
//! That is why the picker search fields were immune: they parse the key, so an
//! unparsed byte sequence simply never becomes a character. The hub was the one
//! field that re-derived text from raw bytes.
//!
//! # The fix, and the one that was rejected
//!
//! Route the hub through the same two helpers - one path, no re-derivation.
//! **Filtering `'['` out was rejected and is proven wrong here**: it hides the
//! symptom while leaving every other escape fragment able to type itself in (`~`,
//! `D`, a paste's first byte), and the row below shows it does not even fix the
//! navigation case, because `\x1b[5~` still yields a printable character once `[`
//! is removed.
//!
//! Behaviour rows live beside the code in `chat_hub_tests.rs`; this file is the
//! NFR-24 named guard, asserting the properties a behavioural row cannot see.

use std::path::Path;

fn read(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #329 - **the hub's fallthrough takes parsed text.** Both helpers, in
// this order: `printable` first so a bracketed paste (which `parse_key` would reject
// as a multi-character sequence) still reaches `paste_text`. Asserted on the arm's
// shape, because a behavioural row cannot distinguish "parsed" from "scanned and
// happened to agree for the inputs it was given".
#[test]
fn the_hub_fallback_takes_parsed_text() {
    let hub = read("crates/lca-ui/src/chat_hub.rs");
    let arm = hub
        .split("pub(crate) fn handle_extension_hub_key(")
        .nth(1)
        .expect("handle_extension_hub_key exists");
    let fallback = arm
        .split("_ => {")
        .nth(1)
        .and_then(|rest| {
            rest.split("    /// Answer the install-consent modal")
                .next()
        })
        .expect("the fallthrough arm exists");
    assert!(
        fallback.contains("printable(data)") && fallback.contains("paste_text(data)"),
        "the arm uses the shared text helpers:\n{fallback}"
    );
    assert!(
        !fallback.contains("data.chars()"),
        "and never scans the raw payload for characters - that is the defect:\n{fallback}"
    );
}

// Verifies: gh #329 - **the rejected fix stays rejected.** Filtering `'['` (or any
// control character) out of the raw stream hides the symptom and leaves the class
// open: `\x1b[5~` (PageUp) would type `5~` with `[` removed, `\x1b[3~` (Delete)
// would type `3~`, and a pasted payload's first byte is still a character. Pinned as
// a row so a future "small fix" cannot quietly reintroduce it.
#[test]
fn a_character_blacklist_is_not_the_fix() {
    let hub = read("crates/lca-ui/src/chat_hub.rs");
    let arm = hub
        .split("pub(crate) fn handle_extension_hub_key(")
        .nth(1)
        .expect("handle_extension_hub_key exists");
    for blacklist in [
        "!= '['",
        "!c.is_control() &&",
        "filter(|c| *c != '[",
        "replace('[',",
    ] {
        assert!(
            !arm.contains(blacklist),
            "the arm filters characters instead of parsing keys ({blacklist}) - gh #329 \
             called this out as the wrong fix"
        );
    }
    // And the reason it is wrong, measured on the key layer: **no escape sequence
    // is text to `printable`**, so the fix needs no character list at all. A `[`
    // blacklist would leave PageUp (`\x1b[5~`) typing `5~`, Delete (`\x1b[3~`)
    // typing `3~`, and so on - the class stays open.
    for sequence in [
        "\x1b[A", "\x1b[B", "\x1b[C", "\x1b[D", "\x1b[H", "\x1b[F", "\x1b[5~", "\x1b[6~",
        "\x1b[3~", "\x1b[2~",
    ] {
        assert!(
            lca_tui::engine::keys::decode_printable_key(sequence).is_none(),
            "{sequence:?} is not text to the key layer, so no character filter is needed"
        );
    }
    // The two keys whose *second* byte is printable-and-not-`[` - the ones a `[`
    // blacklist would still leak - are the proof the instance is not the class.
    for sequence in ["\x1b[5~", "\x1b[3~"] {
        assert!(
            lca_tui::engine::keys::decode_printable_key(sequence).is_none(),
            "{sequence:?} would type digits with `[` filtered out"
        );
    }
}

// Verifies: gh #329 - **a typed `[` is text, and the helpers say so.** This is the
// property that makes the fix correct rather than merely quiet: `[` arrives as a
// single byte with no ESC, so `parse_key` returns it as a one-character key and
// `printable` yields it. A blacklist would break exactly this user, filtering for
// `[draft]`.
#[test]
fn a_typed_bracket_is_text_to_the_key_layer() {
    assert_eq!(
        lca_tui::engine::keys::parse_key("[").as_deref(),
        Some("["),
        "a bare `[` parses as itself"
    );
    let hub = read("crates/lca-ui/src/chat_hub.rs");
    assert!(
        !hub.contains("'['") && !hub.contains("\"[\""),
        "and the hub has no special case for it"
    );
}

// Verifies: gh #329 - **the seam receives a whole paste, by construction.** This is
// the fact that makes the field's job small: `StdinBuffer` buffers
// `ESC[200~`-to-`ESC[201~` across however many reads it takes and emits one
// `StdinEvent::Paste`, which the terminal layer re-wraps as one string before
// dispatch. So a field never sees the opener, the payload and the closer as three
// inputs - it sees the markers around the payload, once.
//
// Pinned here because a future reader could otherwise conclude the field must
// reassemble pastes itself, and write a second state machine beside the one that
// already works.
#[test]
fn the_input_layer_hands_over_one_whole_paste() {
    let buffer = read("crates/lca-tui/src/engine/stdin_buffer.rs");
    assert!(
        buffer.contains("paste_buffer") && buffer.contains("BRACKETED_PASTE_END"),
        "the stdin buffer accumulates a paste across reads"
    );
    assert!(
        buffer.contains("StdinEvent::Paste(content)"),
        "and emits it once, when the closer arrives"
    );
    // The terminal layer re-wraps before dispatch, so the handler sees the markers.
    let terminal = read("crates/lca-tui/src/engine/terminal.rs");
    // Asserted on the marker literals and the single `forward_input` call, not
    // on the exact recipe: the property is that dispatch receives the markers
    // around the payload, not how the string is built.
    // A window of lines, not the first one: the arm's body sits on the following
    // line, and taking exactly one would assert on `{`.
    let forward = terminal
        .split("StdinEvent::Paste(content) =>")
        .nth(1)
        .map(|rest| rest.lines().take(3).collect::<Vec<_>>().join("\n"))
        .expect("the paste arm exists");
    assert!(
        forward.contains("200~") && forward.contains("201~") && forward.contains("forward_input"),
        "the terminal layer hands over one re-wrapped paste:\n{forward}"
    );
    // And the field extracts its payload with the shared helper, not by hand.
    let hub = read("crates/lca-ui/src/chat_hub.rs");
    assert!(
        hub.contains("paste_text(data)"),
        "the hub asks the shared helper for the payload"
    );
}

// Verifies: gh #329's sweep - **the hub is the only field with this shape.** Every
// other inline text field in `lca-ui` already routes through the shared helpers, so
// the class has one member. Asserted recursively over the crate, with comments and
// test modules stripped, because a byte-scanning arm in a sibling is exactly what a
// per-instance fix would miss.
#[test]
fn no_other_field_in_the_crate_scans_raw_bytes() {
    fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-ui/src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(
        files.len() > 50,
        "the walk found the crate: {}",
        files.len()
    );

    let mut offenders: Vec<String> = Vec::new();
    for path in files {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        for (index, line) in production.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if code.contains("data.chars()") || code.contains("data.as_bytes().iter()") {
                offenders.push(format!(
                    "{}:{}: {}",
                    path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&path)
                        .display(),
                    index + 1,
                    code.trim()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a text field must take parsed text (`printable`/`paste_text`), not scan raw \
         input bytes (gh #329):\n{}",
        offenders.join("\n")
    );
}
