//! NFR-24 guard for gh #351 finding 1: the two ways a webfetch body can end, and
//! the two ways its text can be cut.
//!
//! The finding as filed was a **test gap**: `FakeNet::read_body` always answered
//! `Some`, so every row in `extensions/webfetch/tests/webfetch.rs` exited through the
//! input cap and the *complete fetch* path had never run. Closing it exposed the defect
//! underneath, which is why this file exists as well as the behavioural rows:
//!
//! **The input cut was silent.** The read loop stops at 1 MiB, and the only marker
//! added was for the 50 KiB *output* bound. So for any page whose 1 MiB prefix
//! converted to less than 50 KiB of text, the cut left no trace. Measured before the
//! fix, with a 1.4 MB page whose first 1 MiB is one `<script>` block (the shape a real
//! page with a large inline bundle has):
//!
//! ```text
//! DEFECT page=1400067 returned=0 bytes, silent=true out=""
//! ```
//!
//! — an empty string, no marker, and the model reads that as "this page is empty".
//!
//! ## What is pinned here, and why it is not redundant with the behavioural rows
//!
//! The behaviour lives beside the code (`both_cuts_are_reported_and_with_their_own_marker`,
//! `a_page_cut_before_its_content_says_so_instead_of_returning_nothing`,
//! `a_page_that_ends_on_the_cap_is_not_reported_as_cut`), because `webfetch` pulls
//! `html2md` (GPL-3.0+, outside `deny.toml`'s allow list) and therefore **cannot** be a
//! dependency of this crate — the same licensing wall `302-webfetch-interactive-consent.rs`
//! documents. So this file asserts the shape instead: two distinct markers exist, the
//! input one is appended **after** the output bound (a page long in both senses carries
//! both facts), and the "did the page continue" answer comes from a **read**, not from
//! the loop's budget being exhausted.
//!
//! That last property is the one a future simplification would quietly break. An exact
//! fit on the cap loses nothing, so `body.len() >= INPUT_MAX` alone is not evidence of a
//! cut — the behavioural row pins the discrimination, and this one pins that the code
//! still asks the transport rather than inferring.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The crate's code without its doc comments (they narrate the old shapes by name).
fn code(rel: &str) -> String {
    read(rel)
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with("//!") || trimmed.starts_with("///"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of `fetch_via`, which is where both cuts are decided.
fn fetch_via() -> String {
    let source = code("extensions/webfetch/src/lib.rs");
    let start = source
        .find("fn fetch_via(")
        .unwrap_or_else(|| panic!("fetch_via exists"));
    let rest = &source[start..];
    // The function ends at the next top-level `\n}` after the loop.
    let end = rest
        .find("\n/// Truncate to the output bound")
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

// Verifies: gh #351 - two cuts, two markers. One marker for both was how the input
// cut stayed invisible: it only ever appeared when the *text* exceeded the output
// bound, which is a different fact about a different bound.
#[test]
fn the_two_cuts_have_distinct_markers() {
    let source = code("extensions/webfetch/src/lib.rs");
    assert!(
        source.contains("pub const TRUNCATION_MARKER"),
        "the output cut keeps its marker: {source:.400}"
    );
    assert!(
        source.contains("pub const INPUT_TRUNCATION_MARKER"),
        "and the input cut has its own: {source:.400}"
    );
    // Distinct messages, not one constant under two names.
    assert_ne!(
        source
            .split("pub const TRUNCATION_MARKER: &str = ")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .map(str::to_string),
        source
            .split("pub const INPUT_TRUNCATION_MARKER: &str = ")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .map(str::to_string),
        "the two markers must say different things"
    );
}

// Verifies: gh #351 - the input marker is appended **after** `bound`, so a page cut in
// both senses reports both. Appending before would have the output bound truncate the
// marker away, which is the failure that would make this fix a no-op on exactly the
// largest pages.
#[test]
fn the_input_marker_survives_the_output_bound() {
    let body = fetch_via();
    let bounded = body
        .find("bound(&converted)")
        .unwrap_or_else(|| panic!("the output bound runs: {body}"));
    let marked = body
        .find("INPUT_TRUNCATION_MARKER")
        .unwrap_or_else(|| panic!("the input marker is appended: {body}"));
    assert!(
        bounded < marked,
        "the marker follows the bound, so truncation cannot eat it: {body}"
    );
    assert!(
        body.contains("out.push_str(INPUT_TRUNCATION_MARKER)"),
        "appended, not prepended: {body}"
    );
}

// Verifies: gh #351 - "did the page continue" is answered by **reading**, not by the
// loop's budget running out.
//
// This is the distinction the behavioural row
// `a_page_that_ends_on_the_cap_is_not_reported_as_cut` pins from outside. Here it is
// pinned as a shape so a future rewrite cannot collapse it into
// `body.len() >= INPUT_MAX` (which mislabels a complete page as cut) or into
// `while`-condition arithmetic.
#[test]
fn whether_the_page_continued_is_asked_of_the_transport() {
    let body = fetch_via();
    let cut = body
        .split("let cut_at_input")
        .nth(1)
        .unwrap_or_else(|| panic!("the cut is a named decision: {body}"));
    let decision = cut.split(';').next().unwrap_or("");
    assert!(
        decision.contains("read_body"),
        "the answer comes from a read past the cap, not from the budget: {decision}"
    );
    assert!(
        decision.contains("INPUT_MAX"),
        "and it only asks when the cap was actually reached: {decision}"
    );
    assert!(
        decision.contains("!chunk.is_empty()"),
        "an empty chunk is not a continuation: {decision}"
    );
    // A peek that *fails* reads as "no continuation" rather than failing the fetch:
    // the page is already in hand, and erroring out of a 1 MiB read because the
    // connection closed a moment early would be the worse trade.
    assert!(
        decision.contains(".ok()"),
        "a failed peek does not discard the page: {decision}"
    );
}

// Verifies: gh #351 - the doc comment on the read loop's bound says what the loop now
// does. The old one claimed the 1 MiB stop was plain "memory safety for the sandbox",
// which was true and incomplete: it was also a silent content cut.
#[test]
fn the_crate_doc_describes_both_bounds() {
    let source = read("extensions/webfetch/src/lib.rs");
    let doc = source
        .lines()
        .take_while(|line| line.trim_start().starts_with("//!"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        doc.contains("INPUT_TRUNCATION_MARKER"),
        "the crate doc names the input marker: {doc}"
    );
    assert!(
        doc.contains("50 KiB"),
        "and still describes the output bound: {doc}"
    );
}
