//! Gh #304: a headerless `SKILL.md` silently lost its first instruction lines.
//!
//! The defect was in `crates/lca-protocol/src/skills.rs::parse_skill_text`, and its
//! whole shape is in one sentence: every opening line containing a colon was split,
//! an unknown key fell into a `_ => {}` arm commented "reserved, not errors", and
//! `body_start` advanced past the line — so `Note: this repository requires Rust
//! 1.85.` never reached the model and nothing said so.
//!
//! The behavioural rows live beside the parser
//! (`crates/lca-protocol/tests/skills.rs`), and they are fixture-diffed against both
//! skill implementations through `lca-tools` and `extensions/skills`. This file is
//! the NFR-24 named guard, and it pins the three decisions a green suite could
//! otherwise lose without any test going red:
//!
//! 1. **the field list is the whole rule** — a line is header chrome only when its
//!    key is one of the four, so the list is a named constant rather than four
//!    `match` arms;
//! 2. **the unknown-key arm returns the line's own offset**, not the next one —
//!    "terminates the header" and "the line survives" are one fact, and a fix that
//!    did only the first would still lose the text;
//! 3. **the bounded dialect is a separate function**, so the headerless fix cannot
//!    reach through and change the unambiguous form.
//!
//! There is exactly **one** parser. #156 consolidated them (the brief's sweep asked
//! whether a second copy shared the bug); `lca-tools::skills::parse_skill` and
//! `extensions/skills`'s `parse_skill` both delegate to this one, so the row that
//! pins the delegate is the row that proves the fix has one home.

use std::path::PathBuf;

/// The body of one function, found by brace matching from its signature.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut seen = false;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                seen = true;
            }
            b'}' => {
                depth -= 1;
                if seen && depth == 0 {
                    return &source[start..=i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    &source[start..]
}

/// A source file from the workspace root.
fn source(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

// Verifies: gh #304 - the four known fields are one named list, and a line's key is
// looked up against it.
//
// This is the structural half of the fix. Scattered `match key { "name" => …, _ =>
// {} }` arms are what allowed the unknown arm to be harmless-looking while
// swallowing a line: there was nowhere to ask "is this a header field at all?". The
// lookup now answers that question, and the answer is what the headerless dialect
// terminates on.
#[test]
fn the_known_fields_are_one_named_list() {
    let text = source("crates/lca-protocol/src/skills.rs");
    assert!(
        text.contains("const HEADER_KEYS: [&str; 4]"),
        "the fields are a named list"
    );
    for field in ["name", "description", "match", "disable-model-invocation"] {
        assert!(
            text.contains(&format!("\"{field}\"")),
            "the list carries {field}"
        );
    }
    let lookup = function_body(&text, "fn header_field(");
    assert!(
        lookup.contains("HEADER_KEYS"),
        "and the lookup asks the list rather than re-spelling it:\n{lookup}"
    );
    assert!(
        lookup.contains("split_once(':')"),
        "the split is the first colon, so a value keeps its own colons:\n{lookup}"
    );
    assert!(
        !lookup.contains("contains(char::is_whitespace)"),
        "and there is no second, unreachable whitespace rule restating the list:\n{lookup}"
    );
}

// Verifies: gh #304 - the headerless dialect returns the line's **own** offset for a
// non-field line.
//
// `return offset` vs `return offset + line.len() + 1` is the entire defect, and it is
// one token. A guard that only checked "the unknown key ends the header" would pass
// a fix that still ate the line, so this pins the offset arithmetic itself.
#[test]
fn a_non_field_line_starts_the_body_at_its_own_offset() {
    let text = source("crates/lca-protocol/src/skills.rs");
    let body = function_body(&text, "fn parse_headerless(");
    assert!(
        body.contains(
            "else {\n            // Gh #304: not a field, so this is where the body begins"
        ),
        "the non-field arm is the one that carries the fix:\n{body}"
    );
    // The arm's return must be the bare offset: no `+ line.len()`, no `+ 1`.
    let arm = body
        .split("else {")
        .nth(1)
        .unwrap_or_else(|| panic!("the else arm exists:\n{body}"));
    let arm = arm.split('}').next().unwrap_or(arm);
    assert!(
        arm.contains("return offset;"),
        "the line is not consumed:\n{arm}"
    );
    assert!(
        !arm.contains("line.len()"),
        "and no arithmetic advances past it:\n{arm}"
    );
}

// Verifies: gh #304 design point 4 - the bounded dialect is its own function, so the
// headerless fix cannot reach it.
//
// The two dialects differ in exactly one rule (what an unknown line inside the region
// means), and putting them in one loop is how a fix for one silently changes the
// other. Separate functions make the scope of the change visible in the file.
#[test]
fn the_bounded_dialect_is_a_separate_function() {
    let text = source("crates/lca-protocol/src/skills.rs");
    // Counted, not merely present: a duplicated name would satisfy "contains
    // `fn parse_bounded(`" while the dialects had in fact been merged, which is the
    // mutation this row exists to catch.
    assert_eq!(
        text.matches("fn parse_bounded(").count(),
        1,
        "exactly one bounded-dialect function"
    );
    assert_eq!(
        text.matches("fn parse_headerless(").count(),
        1,
        "exactly one headerless-dialect function"
    );
    let bounded = function_body(&text, "fn parse_bounded(");
    assert!(
        bounded.contains("is_fence(line)"),
        "the bounded dialect terminates on the closing fence:\n{bounded}"
    );
    assert!(
        !bounded.contains("HEADER_KEYS") && bounded.contains("header_field(line.trim())"),
        "and it applies only known fields, leaving the rest as reserved chrome:\n{bounded}"
    );
    // The unterminated case is not a header at all.
    assert!(
        bounded.contains("No closing fence"),
        "an unterminated fence means the file is body:\n{bounded}"
    );
}

// Verifies: gh #304's sweep question - **there is one parser**, and both skill
// implementations delegate to it.
//
// #156 was filed about duplicate skill parsers. If a second copy had come back, this
// cycle's fix would have had to land twice, and the next parser bug would land once.
// The row pins the delegation from both sides so a future fork of the logic fails
// here rather than in a user's skill.
#[test]
fn both_skill_implementations_use_the_one_parser() {
    for (rel, what) in [
        ("crates/lca-tools/src/skills.rs", "the host merge"),
        ("extensions/skills/src/lib.rs", "the skills extension"),
    ] {
        let text = source(rel);
        assert!(
            text.contains("lca_protocol::skills::parse_skill_text"),
            "{what} delegates to the shared parser"
        );
        assert!(
            !text.contains("fn parse_skill_text("),
            "{what} defines no parser of its own"
        );
        assert!(
            !text.contains("disable-model-invocation\""),
            "{what} does not re-spell the field list"
        );
    }
}
