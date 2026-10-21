//! GitHub #388 gap 4: `$EDITOR` must not add a phantom blank line.
//!
//! Every POSIX editor terminates the file it saves with a newline — `vim`'s
//! `fixendofline`, `nvim`, `nano` all do — so reading the buffer back verbatim
//! turned a one-line prompt into two lines, the second empty. The user opened
//! the editor to fix a typo and got a message with a blank line they never
//! typed, on every single `$EDITOR` round trip. A trailing blank line is not
//! cosmetic in a prompt: it is sent to the model, it survives in the session
//! log, and `/export` reproduces it.
//!
//! pi's rule is `stripBom(...).replace(/\n$/, "")` — strip a BOM, then **one**
//! trailing newline — and the narrowness is the point. `trim_end()` would pass
//! the "no phantom blank line" test while destroying a prompt that deliberately
//! ends in a blank line (a trailing paragraph, a `here:` line about to be
//! pasted). One terminator only: the file's, not the user's.
//!
//! The behavioural rows live beside the code
//! (`crates/lca-cli/src/tui/editor.rs` `mod tests`, driven through a fake
//! editor that writes byte-exact content).
//! This file is the NFR-24 named guard: it pins the *mechanism* from outside,
//! so a refactor cannot quietly restore the verbatim read or reach for
//! `trim_end` instead.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The `$EDITOR` handoff lives in its own module since it crossed the
/// 1,200-line ceiling out of `hooks.rs`; this guard follows the code.
fn editor_source() -> String {
    let path = root().join("crates/lca-cli/src/tui/editor.rs");
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The body of `normalize_edited_prompt`, from its signature to the closing
/// brace of the function.
fn normalize_body(source: &str) -> &str {
    let start = source
        .find("fn normalize_edited_prompt(")
        .expect("`normalize_edited_prompt` exists — the editor contract has a name");
    let rest = &source[start..];
    let end = rest.find("\n}\n").expect("the function has a body");
    &rest[..end]
}

// Verifies: gh #388 gap 4 - the buffer is read back through the contract, not
// verbatim. This is the defect itself: `read_to_string` straight into the
// prompt is what appended a blank line to every edited message.
#[test]
fn the_buffer_is_read_back_through_the_editor_contract() {
    let source = editor_source();
    let start = source
        .find("fn external_editor_with(")
        .expect("`external_editor_with` exists");
    let rest = &source[start..];
    let end = rest.find("\n}\n").expect("the function has a body");
    let body = &rest[..end];

    assert!(
        body.contains("normalize_edited_prompt("),
        "the buffer is normalized before it reaches the prompt"
    );
    // The old shape, which must not come back: a verbatim read feeding
    // `then_some` directly.
    assert!(
        !body.contains("let edited = std::fs::read_to_string(&path).ok();"),
        "reading the buffer verbatim is the #388 gap 4 defect: every POSIX \
         editor's trailing newline became an empty line in the prompt"
    );
}

// Verifies: gh #388 gap 4 - **one** terminator is stripped, never
// `trim_end()`. A prompt the user deliberately ended with a blank line keeps
// that blank line.
#[test]
fn exactly_one_terminator_is_stripped_not_a_trim() {
    let source = editor_source();
    let body = normalize_body(&source);

    for forbidden in ["trim_end(", "trim_end_matches(", "trim()"] {
        assert!(
            !body.contains(forbidden),
            "`{forbidden}` would pass the phantom-blank-line test while \
             destroying a deliberate trailing blank line; strip exactly one \
             terminator instead"
        );
    }
    assert!(
        body.contains("strip_suffix("),
        "a suffix strip is what removes exactly one terminator"
    );
}

// Verifies: gh #388 gap 4 - CRLF and a lone CR are terminators too, so a
// Windows editor's line ending does not leave a bare carriage return behind
// (which the composer would render as a control character).
#[test]
fn the_windows_and_classic_mac_terminators_are_handled() {
    let source = editor_source();
    let body = normalize_body(&source);
    // The source writes these as Rust escapes: "\r\n" and '\r'.
    assert!(
        body.contains(r#""\r\n""#),
        "`\\r\\n` is one terminator, not an `\\n` with a stray `\\r` before it"
    );
    assert!(
        body.contains(r"'\r'"),
        "a lone `\\r` is a terminator as well"
    );
}

// Verifies: gh #388 gap 4 (pi parity) - a leading BOM is stripped. It is
// invisible in an editor and would otherwise become a zero-width character at
// the start of the user's message.
#[test]
fn a_leading_byte_order_mark_is_stripped() {
    let source = editor_source();
    let body = normalize_body(&source);
    assert!(
        body.contains(r"strip_prefix('\u{feff}')"),
        "pi's `stripBom` is the first thing its editor handoff does; LCA must \
         strip the BOM before considering terminators"
    );
}

// Verifies: gh #388 gap 4 - clearing the prompt and a failed editor stay
// distinguishable. `None` means "no edit" (the caller keeps the text);
// `Some("")` means the user deleted everything (the caller clears it). If
// these collapsed, cancelling an edit and deleting the prompt would look
// alike, and one of them would do the wrong thing.
#[test]
fn a_cleared_prompt_is_distinguishable_from_a_failed_editor() {
    let source = editor_source();
    let start = source
        .find("fn external_editor_with(")
        .expect("`external_editor_with` exists");
    let rest = &source[start..];
    let end = rest.find("\n}\n").expect("the function has a body");
    let body = &rest[..end];

    // A failure is decided by the exit status, separately from the content.
    assert!(
        body.contains("status.success()"),
        "the exit status decides failure, so an empty buffer cannot be \
         mistaken for one"
    );
    // And the content is read unconditionally, so an empty file is `Some("")`
    // rather than being filtered into `None` by a non-empty check.
    assert!(
        !body.contains("filter(|text| !text.is_empty())"),
        "an emptied buffer must survive as `Some(\"\")` — the user cleared the \
         prompt, which is an edit, not a failure"
    );
}

// Verifies: gh #388 gap 4 (fail closed) - non-UTF-8 is a refused edit, never a
// lossily mangled prompt. A silently corrupted message is worse than one the
// user has to retype, so there is no `from_utf8_lossy` in this path.
#[test]
fn undecodable_bytes_fail_closed() {
    let source = editor_source();
    let start = source
        .find("fn external_editor_with(")
        .expect("`external_editor_with` exists");
    let rest = &source[start..];
    let end = rest.find("\n}\n").expect("the function has a body");
    let body = &rest[..end];
    assert!(
        !body.contains("from_utf8_lossy"),
        "lossy conversion would mangle a prompt instead of refusing the edit"
    );
    assert!(
        body.contains("read_to_string"),
        "`read_to_string` errors on invalid UTF-8, which is the refusal"
    );
}
