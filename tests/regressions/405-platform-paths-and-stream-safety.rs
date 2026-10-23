//! gh #405 - platform path variants and streaming-text safety.
//!
//! Two gaps landed (1 and 4); two are deferred with a reason (see the report). This
//! file is the NFR-24 guard: it pins the *shape* of both fixes so a later refactor
//! cannot quietly undo them, and it adds regression guards for the two behaviours
//! the issue says are **already at parity with pi** - which get a guard, never a
//! re-file.
//!
//! # What is here, and why each row exists
//!
//! - **Gap 1:** the screenshot candidate set is enumerated (a `replace` on one
//!   template cannot produce a set this wide), and the resolver refuses to invent a
//!   path - a miss reports what it tried.
//! - **Gap 4:** the real defect was not the split chunk (already safe) but an
//!   escape that never completes reaching the renderer as literal bytes. The
//!   sanitizer is called from `wrap_text_with_ansi`, and it drops only a *trailing*
//!   incomplete sequence.
//! - **At parity, guarded:** the fail-closed hyperlink ladder and the multiplexer
//!   image-unsafety rule. Both already match pi; the rows below are what keep them
//!   matching.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// Code only: comments removed, so prose naming a past mistake cannot trip a guard.
fn code_only(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: gh #405 gap 1 - **the screenshot name set is enumerated.** A single
// `replace`/`strip_prefix` on `"Screen Shot {date} at {time}"` cannot produce
// `Screenshot {date} {time}`, and one moved space is how this class of bug arrives.
#[test]
fn the_screenshot_shapes_are_enumerated_not_derived() {
    let paths = code_only(&read("crates/lca-tools/src/paths.rs"));
    let stems = paths
        .find("const SHOT_STEMS")
        .expect("the stem list exists");
    let body = &paths[stems..];
    let end = body.find("];").unwrap_or(body.len());
    let body = &body[..end];
    for stem in ["Screen Shot", "Screenshot", "Screen Recording"] {
        assert!(
            body.contains(stem),
            "the enumerated set names {stem:?}: {body}"
        );
    }
    // Both date/time joins are produced from every stem.
    assert!(
        paths.contains("at {time}") && paths.contains("{stem} {date} {time}"),
        "both joins are enumerated"
    );
}

// Verifies: gh #405 gap 1 design point 2 - **localised Desktop folders are
// searched**, and `~/Pictures` is a *fallback* (after every Desktop spelling), so
// enabling it never changes which of two real files a user gets.
#[test]
fn localised_folders_are_searched_and_pictures_is_last() {
    let paths = code_only(&read("crates/lca-tools/src/paths.rs"));
    let desktop = paths
        .find("const DESKTOP_DIRS")
        .expect("the Desktop list exists");
    let dbody = &paths[desktop..];
    let dend = dbody.find("];").unwrap_or(dbody.len());
    for local in ["Desktop", "Bureau", "Schreibtisch", "Escritorio"] {
        assert!(
            dbody[..dend].contains(local),
            "the Desktop spellings include {local}"
        );
    }
    // The concat order is Desktop-then-Pictures, which is what makes it a fallback.
    assert!(
        paths.contains("for dir in [DESKTOP_DIRS, PICTURES_DIRS].concat()"),
        "Desktop is searched before Pictures"
    );
}

// Verifies: gh #405 gap 1 design point 3 - **never guess a path that does not
// exist.** The miss type carries the candidates, so a user is shown the search
// rather than pointed at a file that is not there.
#[test]
fn a_miss_carries_the_candidates_it_tried() {
    let paths = code_only(&read("crates/lca-tools/src/paths.rs"));
    assert!(
        paths.contains("pub struct ScreenshotMiss"),
        "the miss type exists"
    );
    assert!(
        paths.contains("fn tried(&self)"),
        "and exposes what it tried"
    );
    assert!(
        paths.contains("not found; tried:"),
        "with wording that lists them"
    );
    // `resolve` alone is not the only entry point: the detailed one is what the
    // user-facing error comes from.
    assert!(
        paths.contains("pub fn resolve_detailed("),
        "the detailed resolver exists"
    );
}

// Verifies: gh #405 gap 1 - **the resolver is actually called by the read path.**
// This is the row that would have caught the first version of this fix: a resolver
// with no production caller passes every unit row and changes nothing for a user.
#[test]
fn the_read_path_calls_the_screenshot_resolver() {
    let lib = code_only(&read("crates/lca-tools/src/lib.rs"));
    let seam = lib
        .find("async fn read(&mut self, call: &ToolCall")
        .expect("the read op exists");
    let body = &lib[seam..];
    let end = body.find("\n    async fn ").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("screenshot_or_literal"),
        "the read op resolves screenshot variants"
    );
    assert!(
        body.contains("miss"),
        "and a failure reports the candidates tried"
    );
    // Only on a literal miss: an existing file must never be second-guessed. That
    // guard lives in the helper, so both halves are asserted.
    let paths = code_only(&read("crates/lca-tools/src/paths.rs"));
    let helper = paths
        .find("pub fn screenshot_or_literal(")
        .expect("the helper exists");
    let hbody = &paths[helper..];
    let hend = hbody.find("\n}\n").unwrap_or(hbody.len());
    assert!(
        hbody[..hend].contains("if target.is_file()"),
        "an existing path is used as-is, so ordinary reads are untouched"
    );
    assert!(
        hbody[..hend].contains("resolve_detailed"),
        "a miss carries the candidates for the error message"
    );
}

// Verifies: gh #405 gap 4 - **the sanitizer is applied where it matters.** Fixing
// the helper without calling it from the render entry would leave the hole open.
#[test]
fn the_render_entry_sanitizes_before_parsing() {
    let text = code_only(&read("crates/lca-tui/src/engine/text.rs"));
    let entry = text
        .find("pub fn wrap_text_with_ansi(")
        .expect("the render entry exists");
    let body = &text[entry..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("sanitize_incomplete_escapes"),
        "the render entry drops an incomplete tail before parsing"
    );
    // And before the normalisation that splits lines, so a dropped tail cannot
    // leave a stray partial line behind.
    let sanitize = body.find("sanitize_incomplete_escapes").unwrap_or(0);
    let normalize = body.find("replace(\"\\r\\n\"").unwrap_or(usize::MAX);
    assert!(
        sanitize < normalize,
        "sanitising happens before line normalisation"
    );
}

// Verifies: gh #405 gap 4 design point 5/8 - **only a trailing incomplete sequence
// is dropped, and the search is bounded.** A sanitizer that stripped every `ESC`
// would break styling; one that scanned without a cap would be an unbounded loop on
// hostile input.
#[test]
fn the_sanitizer_drops_only_a_bounded_trailing_sequence() {
    // Split out of `text.rs` for the ceiling (gate 11); the behaviour is the same.
    let text = code_only(&read("crates/lca-tui/src/engine/text/stream_safety.rs"));
    let seam = text
        .find("pub fn sanitize_incomplete_escapes(")
        .expect("the sanitizer exists");
    let body = &text[seam..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        text.contains("const MAX_TRAILING_SEQUENCE"),
        "the look-back is capped"
    );
    assert!(
        body.contains("MAX_TRAILING_SEQUENCE"),
        "and the sanitizer uses that cap"
    );
    // The drop is `&text[..pos]` from the LAST escape - not a blanket strip.
    assert!(
        body.contains("&text[..pos]"),
        "it truncates at the unterminated sequence rather than filtering"
    );
    assert!(
        body.contains("extract_ansi_code(text, pos).is_some()"),
        "a complete sequence at the tail means nothing is dropped"
    );
}

// Verifies: gh #405 - **at parity, guarded: the hyperlink ladder is fail-closed.**
// The issue says this matches pi and must be guarded, not re-filed. Unknown
// terminal, `screen`, and a non-forwarding tmux client all answer "no", so a URL is
// never silently swallowed by a link the terminal cannot draw.
#[test]
fn hyperlink_detection_stays_fail_closed() {
    let terminal = code_only(&read("crates/lca-tui/src/engine/terminal.rs"));
    let seam = terminal
        .find("fn detect_hyperlinks_with(")
        .expect("the ladder's testable half exists");
    let body = &terminal[seam..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    let body = &body[..end];
    // An unknown terminal falls through to `false`, not `true`.
    assert!(
        body.trim_end().ends_with("false"),
        "the ladder's default is off (fail-closed): {body}"
    );
    assert!(
        body.contains("screen"),
        "screen is explicitly off, not probed"
    );
    // The override still wins, which is the receipt path.
    assert!(
        terminal.contains("LCA_HYPERLINKS"),
        "the documented override is still honoured"
    );
}

// Verifies: gh #405 - **at parity, guarded: images under a multiplexer are
// `None`.** pi calls images "unreliable under tmux" and a wrong guess is invisible
// data loss, so the rule is a blanket refusal rather than a guess.
#[test]
fn image_protocol_is_none_under_a_multiplexer() {
    let image = code_only(&read("crates/lca-tui/src/widgets/image.rs"));
    let seam = image
        .find("pub fn detect_image_protocol_with(")
        .expect("the injected-env detector exists");
    let body = &image[seam..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    let body = &body[..end];
    for marker in ["TMUX", "STY", "tmux", "screen"] {
        assert!(
            body.contains(marker),
            "the multiplexer guard names {marker}: {body}"
        );
    }
    // The refusal returns `None`, not a best-guess protocol.
    assert!(
        body.contains("return ImageProtocol::None"),
        "and it refuses rather than guessing"
    );
}
