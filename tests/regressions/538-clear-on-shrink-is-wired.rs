//! GitHub #538: `terminal.clear_on_shrink` was implemented but unreachable.
//!
//! ## The gap this closes
//!
//! Gh #372 bug 4 gave the scrollback renderer pi's `terminal.clearOnShrink`
//! (`MainScreenRenderer::set_clear_on_shrink`, with rows for clear / grow /
//! default). The config key already existed, parsed, defaulted and merged. What
//! was missing was one line: nothing ever *called* the setter in production, so
//! the documented setting was inert and `docs/configuration.md` had to say so.
//!
//! The gap survived a cycle precisely because the renderer's own rows set the
//! flag by hand (`renderer.set_clear_on_shrink(true)`), which proves the
//! renderer and nothing about the path to it. So this file pins the *wiring*,
//! not the renderer:
//!
//! 1. `lca-cli` fills the tunable from config (the value travels),
//! 2. `lca-ui` hands the tunable to the main-screen renderer on every
//!    construction path (nothing gets missed when a second one appears),
//! 3. the doc row stops claiming the key is inert.
//!
//! Verifies: NFR-24 (a released defect gets a named guard) and gh #538.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// A source file without its test modules, so a row cannot be satisfied by a
/// test's own spelling of the seam.
fn production_source(rel: &str) -> String {
    let source = read(rel);
    match source.find("\n#[cfg(test)]") {
        Some(at) => source[..at].to_string(),
        None => source,
    }
}

/// The body of one function, from its signature to the next line that closes it
/// at the same indent.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |at| at + 3);
    &rest[..end]
}

// Verifies: gh #538 - the value travels. The tunable the interface reads is
// filled from the merged config, so a config file (or an `LCA_` variable) reaches
// the renderer at all. This is the line that was missing.
#[test]
fn the_cli_fills_the_tunable_from_the_configured_value() {
    let source = production_source("crates/lca-cli/src/tui/hooks.rs");
    let body = function_body(&source, "pub(super) fn display_tuning(");
    assert!(
        body.contains("clear_on_shrink: config.terminal_clear_on_shrink()"),
        "the tunable must be filled from the configured value, or the key stays inert \
         (the whole of gh #538): {body}"
    );
}

// Verifies: gh #538 - there is exactly one place a production `DisplayTuning` is
// built. The bug was a `DisplayTuning` field that nothing in production set; a
// second construction site is how the same class of gap comes back.
#[test]
fn one_production_tuning_and_no_second_fill() {
    let mut sites = Vec::new();
    for rel in [
        "crates/lca-cli/src/tui/hooks.rs",
        "crates/lca-cli/src/tui/mod.rs",
        "crates/lca-cli/src/tui/display.rs",
        "crates/lca-cli/src/tui/runner.rs",
        "crates/lca-cli/src/tui/startup.rs",
    ] {
        let source = production_source(rel);
        for (index, _) in source.match_indices("DisplayTuning {") {
            // A *construction* is what this row counts, so a return type in a
            // signature (`-> lca_ui::state::DisplayTuning {`) is not one: the
            // brace there opens the function body.
            let line_before = source[..index]
                .rsplit_once('\n')
                .map_or(&source[..index], |(_, tail)| tail);
            if line_before.contains("->") {
                continue;
            }
            sites.push(format!("{rel}@{index}"));
        }
    }
    assert_eq!(
        sites.len(),
        1,
        "one production DisplayTuning fill, so the two the SRDD warns about cannot \
         drift: {sites:?}"
    );
    assert!(
        sites[0].starts_with("crates/lca-cli/src/tui/hooks.rs"),
        "and it is the one the interface's hook calls: {sites:?}"
    );
}

// Verifies: gh #538 - the seam actually reaches the renderer. `lca-ui` builds the
// main-screen renderer and then applies the tunables to it; the apply must cover
// the main screen, not only the alt screen, or the setting reaches no renderer at
// all (which was the state before this change).
#[test]
fn applying_the_tunables_sets_clear_on_shrink_on_the_main_screen() {
    let source = production_source("crates/lca-ui/src/run_selection.rs");
    let body = function_body(&source, "pub(super) fn apply_screen_tuning(");
    assert!(
        body.contains("Screen::Main(main)") && body.contains("set_clear_on_shrink"),
        "the apply seam must set it on the main screen: {body}"
    );
}

// Verifies: gh #538 - every construction path is followed by the apply, so a
// second renderer (the `/fullscreen` switch back) cannot silently miss the
// setting. Counted rather than assumed: two `Screen::Main(MainScreenRenderer::new())`
// sites, and each one has an `apply_screen_tuning` after it inside the same
// function.
#[test]
fn every_main_screen_construction_is_followed_by_the_apply() {
    let source = production_source("crates/lca-ui/src/run.rs");
    let constructions = source
        .match_indices("Screen::Main(MainScreenRenderer::new())")
        .count();
    assert_eq!(
        constructions, 2,
        "the initial screen and the fullscreen switch back - a new site needs this \
         row updated, which is the point"
    );
    let applies = source.matches("apply_screen_tuning(").count();
    assert_eq!(
        applies, 2,
        "one apply per construction: {applies} applies for {constructions} sites"
    );
}

// Verifies: gh #538 - the doc row stops saying the key is inert. A repo that has
// twice fixed docs describing a limitation that no longer existed should not
// leave a third behind; this row fails if the caveat comes back or the row loses
// the wiring it now has.
#[test]
fn the_doc_row_no_longer_claims_the_key_is_inert() {
    let docs = read("docs/configuration.md");
    let row = docs
        .lines()
        .find(|line| line.starts_with("| `terminal.clear_on_shrink`"))
        .expect("the key has a row in the configuration table");
    assert!(
        !row.contains("inert"),
        "the key works now, so the row must not call it inert: {row}"
    );
    assert!(
        !row.contains("No effect until"),
        "and must not defer the effect to a future cycle: {row}"
    );
    assert!(
        row.contains("clears") || row.contains("Clears"),
        "the row says what it does: {row}"
    );
    assert!(
        row.contains("| `false` |"),
        "and its default is still off (FR-CFG-1's inspector reads it): {row}"
    );
}
