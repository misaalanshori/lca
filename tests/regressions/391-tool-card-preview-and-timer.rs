//! GitHub #391: three rendering gaps in the collapsed tool card.
//!
//! 1. Previews sliced **logical** lines, so one 4,000-character minified line
//!    wrapped into ~50 visual rows inside a card that called itself collapsed
//!    — and the five kept lines were the boilerplate at the head while the
//!    pass/fail summary sat at the tail.
//! 2. No live timer: a 90 s `cargo test` showed a frozen card, because `Took`
//!    only appeared once the call settled.
//! 3. `!` and `!!` looked identical, although one's output reaches the model
//!    and the other's does not.
//!
//! The behavioural rows live beside the code
//! (`crates/lca-ui/src/transcript/{preview,timer}_tests.rs`). This file is the
//! NFR-24 named guard: it pins the *mechanism* from outside, so a refactor
//! that moves the logic out of `include_str!`'s reach fails loudly instead of
//! leaving those rows quietly checking nothing.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

#[test]
fn the_preview_wraps_before_it_slices() {
    let preview = read("crates/lca-ui/src/transcript/preview.rs");
    let build = preview
        .find("pub(super) fn build_preview")
        .expect("build_preview exists: gh #391 gap 1");
    let body = &preview[build..];
    let end = body.find("\n}\n").expect("build_preview has a body");
    let body = &body[..end];

    let wrap = body
        .find("wrap_text_with_ansi")
        .expect("the preview wraps the text");
    let slice = body
        .find("split_off")
        .or_else(|| body.find("truncate("))
        .expect("the preview slices visual rows");
    assert!(
        wrap < slice,
        "the wrap must come BEFORE the slice; slicing logical lines first is \
         the defect - one long line then blows the visual budget by 10x"
    );
    assert!(
        !body.contains(".lines().take("),
        "no logical-line slicing: that is the pre-fix shape"
    );
}

#[test]
fn shells_keep_the_tail_and_their_marker_sits_above_it() {
    let preview = read("crates/lca-ui/src/transcript/preview.rs");
    let keeps = preview
        .find("pub(super) fn keeps_tail")
        .expect("keeps_tail exists");
    let body = &preview[keeps..];
    let end = body.find("\n}\n").expect("keeps_tail has a body");
    assert!(
        body[..end].contains("\"bash\" | \"shell\""),
        "shell output keeps its tail: the summary is at the bottom"
    );

    let render = read("crates/lca-ui/src/transcript/render.rs");
    assert!(
        render.contains("built.marker_above"),
        "the renderer must honour which side the omission marker goes on"
    );
    assert!(
        render.contains("marker(built.omitted)"),
        "and there must be exactly one marker construction, not two copies"
    );
}

#[test]
fn the_timer_reads_a_start_instant_and_freezes_on_a_recorded_duration() {
    let modrs = read("crates/lca-ui/src/transcript/mod.rs");
    assert!(
        modrs.contains("started_at: Option<std::time::Instant>"),
        "the card records when its call started (gh #391 gap 2)"
    );
    assert!(
        modrs.contains("pub fn has_running_tool(&self) -> bool"),
        "the tick needs to know whether anything is ticking, or it repaints \
         an idle transcript once a second forever"
    );

    let render = read("crates/lca-ui/src/transcript/render.rs");
    // A recorded duration wins, and a running card falls back to the clock:
    // that ordering is what makes a replayed card show its exact time.
    let took = render
        .find("if let Some(ms) = duration_ms")
        .expect("the recorded duration is checked first");
    let elapsed = render
        .find("Elapsed {}")
        .expect("a running card shows Elapsed");
    assert!(
        took < elapsed,
        "duration_ms must be consulted before the live clock, or a settled \
         card would animate a count that already finished"
    );
    assert!(
        render.contains("format_duration_ms"),
        "pi's tiers live in one formatter"
    );
}

#[test]
fn the_two_manual_modes_are_distinguishable_without_colour() {
    let render = read("crates/lca-ui/src/transcript/render.rs");
    let style = render
        .find("(true, true) => Role::Dim")
        .expect("`!!` draws dim (gh #391 gap 3)");
    let marker = render
        .find("if *excluded { \"!!\" } else { \"!\" }")
        .expect("the textual marker distinguishes the two modes");
    // NFR-28: colour is never the only signal, so the marker is not optional.
    assert!(
        style < marker || marker < style,
        "both the role and the marker must be present"
    );
}

#[test]
fn the_truncation_footer_is_stripped_before_the_preview_sees_it() {
    let preview = read("crates/lca-ui/src/transcript/preview.rs");
    let strip = preview
        .find("pub(super) fn strip_truncation_footer")
        .expect("the strip helper exists (gh #391 gap 4)");
    let body = &preview[strip..];
    let end = body.find("\n}\n").expect("the helper has a body");
    // LCA's own shape is what actually reaches the renderer; matching only
    // pi's spelling would have left the real footer in place.
    assert!(
        body[..end].contains("full output: "),
        "the strip must match the footer shape this tree writes \
         (`lca-tools`' `\\n[full output: N bytes, attachment <hash>]`)"
    );

    let render = read("crates/lca-ui/src/transcript/render.rs");
    assert!(
        render.contains("strip_truncation_footer(result)"),
        "the renderer strips the footer before previewing"
    );
}
