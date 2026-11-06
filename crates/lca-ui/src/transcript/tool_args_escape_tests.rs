//! gh #405 gap 4: a tool card's headline must show the argument the model sent.
//!
//! `json_string_field` pulls one string field out of a tool call's arguments by hand.
//! Its `\\u` branch skipped the four hex digits and pushed `U+FFFD`, so **every
//! non-ASCII character in a tool argument became a replacement character in the card's
//! headline**. JSON escapes non-ASCII by default, so this was not an edge case: it was
//! every CJK, Cyrillic, Greek and emoji argument.
//!
//! Measured live, before the fix — a shell command whose argument is a Japanese string
//! painted as this, while the command itself ran correctly and its output was right:
//!
//! ```text
//! > shell printf '�����������' | od -An -v -tu1 | ...
//! ```
//!
//! The card looked like the model had sent garbage. It had not.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code.

use super::format_tool_args;

// Verifies: gh #405 gap 4 - **a CJK argument survives to the headline.**
//
// The exact shape `serde_json` produces for a non-ASCII string: every character is a
// `\\uXXXX` escape. This is the case the live pane showed.
#[test]
fn a_cjk_argument_is_decoded_to_text() {
    let args = r#"{"command":"printf '\u65e5\u672c\u8a9e'"}"#;
    assert_eq!(
        format_tool_args("shell", args),
        "printf '日本語'",
        "the headline shows the argument, not replacement characters"
    );
    assert!(
        !format_tool_args("shell", args).contains('\u{fffd}'),
        "and no U+FFFD anywhere in it"
    );
}

// Verifies: gh #405 gap 4 - **a surrogate pair decodes to one character.** An emoji is
// outside the BMP, so JSON writes it as two escapes; emitting half of a pair is how
// mojibake starts.
#[test]
fn a_surrogate_pair_decodes_to_one_character() {
    let args = r#"{"pattern":"\ud83c\udf89 party"}"#;
    assert_eq!(
        format_tool_args("grep", args),
        "/🎉 party/",
        "the pair becomes the emoji, and the grep summary keeps its slashes"
    );
}

// Verifies: gh #405 gap 4 - **a lone surrogate is one replacement, not two.**
//
// Input that cannot be text is the honest case for `U+FFFD`; the row exists so the
// decoder cannot start emitting arbitrary bytes for half a pair, which is worse than a
// marker.
#[test]
fn a_lone_surrogate_is_one_replacement() {
    let args = r#"{"command":"a\ud83cb"}"#;
    let out = format_tool_args("shell", args);
    assert_eq!(
        out.matches('\u{fffd}').count(),
        1,
        "one marker for the malformed escape, not a run: {out:?}"
    );
    assert!(
        out.contains('a') && out.contains('b'),
        "and the text around it survives: {out:?}"
    );
}

// Verifies: gh #405 gap 4 - **the escapes that are not `\\u` still work**, and a plain
// ASCII argument is byte-identical to what it was. The decoder replaced a branch, not a
// behaviour: `\\n`, `\\t`, `\\r` and an escaped quote all keep their meaning.
#[test]
fn ascii_arguments_and_the_simple_escapes_are_unchanged() {
    for (label, args, expected) in [
        ("plain", r#"{"command":"cargo test"}"#, "cargo test"),
        ("newline", r#"{"command":"a\nb"}"#, "a\nb"),
        ("tab", r#"{"command":"a\tb"}"#, "a\tb"),
        ("quote", r#"{"command":"echo \"hi\""}"#, "echo \"hi\""),
        ("backslash", r#"{"command":"a\\b"}"#, "a\\b"),
        ("backspace", r#"{"command":"a\bb"}"#, "a\u{8}b"),
    ] {
        assert_eq!(format_tool_args("shell", args), expected, "{label}");
    }
}
