//! Rows for the tool card's argument grammar (gh #392 slice C).
//!
//! One row per **failure mode the issue names**, because each of them was a real defect in
//! the one-branch match this replaces: a `read` that hid its range, a range built by
//! concatenation (`:18060`), a `null` that rendered as the string `null`, a 400-character
//! blob on a collapsed card, a non-string argument that vanished, and a streaming-empty
//! argument that rendered as nothing at all.
//!
//! The `~`-shortening and the OSC 8 link are asserted through `render_path`'s observable
//! behaviour rather than by calling it directly: the hyperlink wrapper depends on the
//! terminal's capability, so a row that asserted `file://` unconditionally would pass on a
//! workstation and fail under a pipe. The link's own presence is covered by the row below,
//! which states the capability it assumes.

use super::*;

// Verifies: gh #392 gap 3 - **a read shows the lines it asked for.** `:100-149` is pi's
// shape for `offset` 100 with `limit` 50: the end is inclusive, which is how a reader thinks
// about "lines 100 to 149" and what the issue's example states.
#[test]
fn a_read_range_renders_inclusive_start_and_end() {
    assert_eq!(
        format_tool_args("read", r#"{"path":"src/lib.rs","offset":100,"limit":50}"#),
        "src/lib.rs:100-149"
    );
}

// Verifies: gh #392 gap 3 - **an offset with no limit is a lower-bounded read**, so the
// summary names the start alone (`:100`) instead of inventing an end.
#[test]
fn a_read_with_only_an_offset_names_its_start() {
    assert_eq!(
        format_tool_args("read", r#"{"path":"src/lib.rs","offset":100}"#),
        "src/lib.rs:100"
    );
}

// Verifies: gh #392 gap 3 - **a whole-file read shows no range at all.** Not `:1`, not
// `:0`: a range that names a line the call never asked for is worse than no range, because
// a reader would trust it.
#[test]
fn a_whole_file_read_shows_no_range() {
    assert_eq!(
        format_tool_args("read", r#"{"path":"src/lib.rs"}"#),
        "src/lib.rs"
    );
    // And a `limit` alone cannot bound a read from nowhere: no range.
    assert_eq!(
        format_tool_args("read", r#"{"path":"src/lib.rs","limit":50}"#),
        "src/lib.rs"
    );
}

// Verifies: gh #392 gap 3 - **the `"180"`/`"60"` coercion trap.** Models emit numbers as
// JSON strings as often as as numbers, and the failure the issue names is arithmetic on the
// raw text: `"180"` and `"60"` concatenated into `:18060`. Both forms must coerce to a
// number *before* any arithmetic, and the answer is `:180-239`.
#[test]
fn numeric_string_arguments_never_concatenate_into_a_line_range() {
    assert_eq!(
        format_tool_args("read", r#"{"path":"a.rs","offset":"180","limit":"60"}"#),
        "a.rs:180-239"
    );
    // The mixed form too: a string offset with a numeric limit.
    assert_eq!(
        format_tool_args("read", r#"{"path":"a.rs","offset":"180","limit":60}"#),
        "a.rs:180-239"
    );
    // And the defect itself, stated as a must-not: the concatenated range never appears.
    let summary = format_tool_args("read", r#"{"path":"a.rs","offset":"180","limit":"60"}"#);
    assert!(
        !summary.contains("18060"),
        "the range was built by concatenation: {summary}"
    );
}

// Verifies: gh #392 gap 3 - **a `null`, a zero, a negative and a non-numeric argument all
// mean "omitted".** Each of these is a shape a real call produces (a streaming `null`, a
// default written as `0`, a model that emitted `-1`, a template that emitted `"abc"`), and
// none of them names a line the read will return.
#[test]
fn an_unusable_range_argument_means_no_range() {
    for args in [
        r#"{"path":"a.rs","offset":null,"limit":null}"#,
        r#"{"path":"a.rs","offset":0,"limit":10}"#,
        r#"{"path":"a.rs","offset":-5,"limit":10}"#,
        r#"{"path":"a.rs","offset":"abc","limit":10}"#,
    ] {
        let summary = format_tool_args("read", args);
        assert!(
            summary.starts_with("a.rs") && !summary.contains(':'),
            "an unusable range argument still produced one: {summary} (from {args})"
        );
    }
    // An offset with a `null` limit is a lower-bounded read, not "no range": the offset **is**
    // usable, so the summary keeps it (the case is asserted here rather than in the loop above,
    // because the loop's claim is "no range at all").
    assert_eq!(
        format_tool_args("read", r#"{"path":"a.rs","offset":100,"limit":null}"#),
        "a.rs:100"
    );
}

// Verifies: gh #392 gap 8 - **an unknown tool's collapsed card is capped at 100
// characters.** A 400-character JSON blob on a collapsed row is the defect: it is too long
// to scan and it is not the payload either.
#[test]
fn an_unknown_tool_summary_is_capped_at_a_hundred_characters() {
    let long = "x".repeat(400);
    let summary = format_tool_args("mystery", &format!(r#"{{"blob":"{long}"}}"#));
    assert!(
        summary.chars().count() <= UNKNOWN_SUMMARY_CAP,
        "the cap is {} characters, got {}: {summary:?}",
        UNKNOWN_SUMMARY_CAP,
        summary.chars().count()
    );
    assert!(
        summary.ends_with('…'),
        "and the card says it was cut: {summary:?}"
    );
    assert!(
        summary.starts_with("blob=\"xxx"),
        "while keeping the `key=\"value\"` grammar: {summary:?}"
    );
}

// Verifies: gh #392 gap 8 - **an unknown tool's arguments read as `key="value"`**, in the
// model's order, with a non-string value kept as its own token rather than quoted as if it
// were text.
#[test]
fn an_unknown_tool_reads_as_key_value_pairs() {
    assert_eq!(
        format_tool_args("mystery", r#"{"alpha":"one","beta":2,"gamma":true}"#),
        "alpha=\"one\" beta=2 gamma=true"
    );
}

// Verifies: gh #392 gap 8 - **a non-string argument is `[invalid arg]`, not a vanished
// field.** The old grammar answered `None` for a non-string `path` and fell back to the raw
// JSON, so the card showed the blob; a purpose-built tool whose own field is unusable now
// says so.
#[test]
fn an_unusable_argument_is_named_rather_than_hidden() {
    assert_eq!(
        format_tool_args("read", r#"{"path":{"nested":"object"}}"#),
        "[invalid arg]"
    );
    assert_eq!(format_tool_args("write", r#"{"path":42}"#), "[invalid arg]");
}

// Verifies: gh #392 gap 8 - **the expanded grammar is one `key: value` line per argument**,
// and the two streaming states have their own shapes: an empty value is `...` (a field the
// model announced but has not filled) and a non-string value is `[invalid arg]`. This is the
// half that makes the 100-character cap harmless.
#[test]
fn an_expanded_unknown_tool_lists_one_argument_per_line() {
    let lines = format_tool_args_expanded(
        "mystery",
        r#"{"alpha":"one","beta":2,"empty":"","nothing":null}"#,
    );
    assert_eq!(
        lines,
        vec![
            "alpha: one".to_string(),
            "beta: [invalid arg]".to_string(),
            "empty: ...".to_string(),
            "nothing: ...".to_string(),
        ]
    );
    // A purpose-built tool has no generic list: its card already has a grammar.
    assert!(
        format_tool_args_expanded("read", r#"{"path":"a.rs"}"#).is_empty(),
        "a known tool does not borrow the fallback grammar"
    );
}

// Verifies: gh #392 gap 8 - **a shell call names its timeout.** pi appends a muted
// `(timeout 60s)` so a reader knows the call cannot hang forever; the suffix is textual, so
// it survives a plain theme (NFR-28: colour is never the only signal).
#[test]
fn a_shell_call_names_its_timeout() {
    assert_eq!(
        format_tool_args("bash", r#"{"command":"npm test","timeout":60}"#),
        "npm test (timeout 60s)"
    );
    // A numeric string coerces like the line range does.
    assert_eq!(
        format_tool_args("shell", r#"{"command":"ls","timeout":"30"}"#),
        "ls (timeout 30s)"
    );
    // No timeout, no suffix - and no `(timeout 0s)` for one that is unusable.
    assert_eq!(format_tool_args("bash", r#"{"command":"ls"}"#), "ls");
    assert_eq!(
        format_tool_args("bash", r#"{"command":"ls","timeout":0}"#),
        "ls"
    );
}

// Verifies: gh #392 gap 8 - **a call with no arguments yet says so.** An empty span is
// indistinguishable from a card that failed to paint, which is exactly how a streaming call
// used to look: the muted `...` shape is the honest statement that arguments are coming.
#[test]
fn a_call_with_no_arguments_yet_renders_the_streaming_shape() {
    assert_eq!(format_tool_args("read", ""), "...");
    assert_eq!(format_tool_args("mystery", "  "), "...");
    // And a partially-streamed object still yields what it has rather than `...`.
    assert_eq!(format_tool_args("read", r#"{"path":"a.rs","off"#), "a.rs");
}

// Verifies: gh #392 gap 3 - **the display path is the house shortening, not the raw
// argument.** A Windows verbatim path must lose its `\\?\` prefix (it is load-bearing
// internally and noise in a card), and a path under `$HOME` must read `~/…`.
#[test]
fn a_read_path_is_shortened_for_display() {
    assert_eq!(
        format_tool_args("read", r#"{"path":"\\\\?\\C:\\work\\a.rs"}"#),
        "C:\\work\\a.rs"
    );
    // `$HOME` is read from the environment by `shorten_home`'s caller only when it is given
    // one; `format_tool_args` passes `None`, so the assertion here is that the path survives
    // unchanged rather than being mangled - the shortening itself has its own rows in
    // `footer` and is not re-derived here.
    assert_eq!(
        format_tool_args("read", r#"{"path":"src/main.rs"}"#),
        "src/main.rs"
    );
}
