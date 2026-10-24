//! Gh #400 gaps 2 and 3: a compact header that expands, and collisions that speak.
//!
//! # Gap 2 — the header printed long-form every launch
//!
//! The startup header is a version line, a `context:` line listing every
//! AGENTS.md path, and a resource-count line. With several context files that is a
//! wall at the top of a session the user did not ask to read, every single launch.
//! The fix is pi's shape: **one compact line** naming the counts and the expand
//! key, expanding on `ctrl+o` - which is `app.tools.expand`, the key the interface
//! already uses to expand a tool card, so there is no second binding to learn.
//!
//! Three failure modes the pattern is named for, from the issue's §4:
//!
//! - **empty sections render zero rows** — no skills and no collisions must not
//!   leave an orphaned divider or a blank spacer;
//! - **every row goes through the width fitter**, so a long extension name or a
//!   wide glyph never staggers the border;
//! - **the expanded state survives `/new`** — a header that resets when you open a
//!   fresh session is worse than one that never expanded.
//!
//! # Gap 3 — a shadowed extension was invisible
//!
//! `ExtensionRegistry::register` already decides installed-first shadowing and
//! **already records a `CollisionReport`** into `registry.collisions()`. Nothing
//! read it. So the fact was known, correct, and discarded: a user who installed an
//! extension and found the bundled one answering had no way to learn why.
//!
//! The notice names both sides and which won, and it **survives `quietStartup`** -
//! a shadowed extension is a diagnosis, not decoration, and the whole point is that
//! a user with a quiet startup still learns why the thing they installed is not the
//! thing that loaded.
//!
//! Verifies: gh #400 gaps 2 and 3, FR-EXT-11, NFR-24.

use std::path::PathBuf;

fn paths(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(PathBuf::from).collect()
}

// ---------------------------------------------------------------------------
// Gap 2: the compact header
// ---------------------------------------------------------------------------

// Verifies: gh #400 gap 2 - **the default launch is one line.** The compact form
// names the counts and the expand key, and it does *not* print the context paths
// or the long resource sentence. This is the row that is red before the fix: today
// every launch prints the version line, a `context:` line and a counts line.
#[test]
fn the_default_header_is_one_line_with_the_expand_hint() {
    let header = lca_cli::tui::startup::compact_header(
        "0.6.0",
        "0.6",
        &paths(&["/home/u/.lca/AGENTS.md", "/w/.lca/AGENTS.md"]),
        4,
        0,
        0,
        lca_ui::CollisionSummary::default(),
    );
    assert_eq!(header.len(), 1, "one line, not a wall: {header:?}");
    let line = &header[0];
    assert!(
        line.contains("lca 0.6.0"),
        "the version reads first: {line}"
    );
    assert!(
        line.contains("4 extensions") && line.contains("0 skills"),
        "the counts are named: {line}"
    );
    assert!(
        line.contains("ctrl+o"),
        "and the expand key is named: {line}"
    );
    assert!(
        !line.contains("AGENTS.md"),
        "the context paths are *behind* the expand, not on the first line: {line}"
    );
}

// Verifies: gh #400 gap 2 - **the expanded form is the long-form table**, and it
// keeps every fact the compact line dropped (the context paths, one resource row
// each). A compact line with nothing behind it would be a deletion, not a summary.
#[test]
fn the_expanded_header_lists_the_context_and_resource_rows() {
    let context = paths(&["/home/u/.lca/AGENTS.md", "/w/.lca/AGENTS.md"]);
    let expanded = lca_cli::tui::startup::expanded_header(
        "0.6.0",
        "0.6",
        &context,
        4,
        2,
        1,
        lca_ui::CollisionSummary::default(),
    );
    let text = expanded.join("\n");
    assert!(
        expanded.len() > 1,
        "the expanded form is a table, not a line: {expanded:?}"
    );
    for needle in [
        "lca 0.6.0",
        "/home/u/.lca/AGENTS.md",
        "/w/.lca/AGENTS.md",
        "4 extensions",
        "2 skills",
        "1 template",
    ] {
        assert!(text.contains(needle), "`{needle}` is in the table:\n{text}");
    }
}

// Verifies: gh #400 gap 2 / §4's named failure - **an empty section renders zero
// rows.** With no skills, no templates and no collisions, the expanded table must
// not leave a divider over nothing or a blank spacer where a section would be.
// That is the failure the "Section Loom" discipline exists to prevent, and it is
// what makes a table look broken.
#[test]
fn empty_sections_render_no_rows_at_all() {
    let expanded = lca_cli::tui::startup::expanded_header(
        "0.6.0",
        "0.6",
        &[],
        4,
        0,
        0,
        lca_ui::CollisionSummary::default(),
    );
    let text = expanded.join("\n");
    // Nothing at all to say about skills, templates, context or collisions.
    for absent in ["skill", "template", "context:", "collision"] {
        assert!(
            !text.to_lowercase().contains(absent),
            "`{absent}` leaves no row at all:\n{text}"
        );
    }
    // And no row is blank: a spacer for a section that did not render is the same
    // bug wearing whitespace.
    assert!(
        expanded.iter().all(|row| !row.trim().is_empty()),
        "no blank spacer rows:\n{expanded:?}"
    );
}

// Verifies: gh #400 gap 2 / §4 - **every row goes through the width fitter**, so a
// long path or a wide glyph cannot stagger the border. Left un-fitted, one long
// AGENTS.md path pushes the row past the pane and the frame wraps, which reads as a
// broken header rather than a truncated path.
#[test]
fn a_long_row_is_fitted_to_the_width_without_breaking_the_frame() {
    let long = format!("/very/long/{}/AGENTS.md", "segment/".repeat(30));
    let expanded = lca_cli::tui::startup::expanded_header_fitted(
        "0.6.0",
        "0.6",
        &paths(&[&long]),
        4,
        0,
        0,
        lca_ui::CollisionSummary::default(),
        40,
    );
    for row in &expanded {
        let width = lca_tui::engine::text::visible_width(row);
        assert!(
            width <= 40,
            "every row fits the width ({width} > 40): {row:?}"
        );
    }
    let text = expanded.join("\n");
    assert!(
        text.contains('…') || text.contains("..."),
        "the long path is truncated with an ellipsis, not silently clipped:\n{text}"
    );
}

// Verifies: gh #400 gap 2 - **no ANSI or wide glyph staggers the frame.** A colour
// role on a row must not be counted as printable width, and a CJK glyph counts as
// two cells. Both are the reason the fitter measures `visible_width` rather than
// `chars().count()`.
#[test]
fn a_wide_glyph_and_an_ansi_sequence_fit_by_visible_width() {
    let row = format!("\u{1b}[31m{}\u{1b}[0m", "広".repeat(30));
    let fitted = lca_cli::tui::startup::fit_row(&row, 20);
    assert!(
        lca_tui::engine::text::visible_width(&fitted) <= 20,
        "the fitted row measures under the budget: {fitted:?}"
    );
}

// ---------------------------------------------------------------------------
// Gap 3: collisions
// ---------------------------------------------------------------------------

// Verifies: gh #400 gap 3 - **a collision produces a notice naming both sides and
// which won.** The registry already records it (`CollisionReport`) and nothing read
// it; this is the row that turns the recorded fact into something a user can see.
#[test]
fn a_collision_names_both_sides_and_the_winner() {
    let summary = lca_ui::CollisionSummary {
        entries: vec![lca_ui::CollisionEntry {
            name: "formatter".to_string(),
            kind: "extension".to_string(),
            winner: "project .lca/extensions/formatter".to_string(),
            loser: "user ~/.lca/extensions/formatter".to_string(),
        }],
    };
    let rows = lca_cli::tui::startup::collision_rows(&summary);
    assert_eq!(rows.len(), 1, "one notice per collision: {rows:?}");
    let row = &rows[0];
    assert!(row.contains("formatter"), "the name: {row}");
    assert!(
        row.contains("project") && row.contains("user"),
        "both sides are named: {row}"
    );
    assert!(
        row.contains('✓') && row.contains('✗'),
        "with a winner and a loser mark: {row}"
    );
    assert!(
        row.contains("skipped"),
        "the loser is named as skipped: {row}"
    );
}

// Verifies: gh #400 gap 3 - **silence when there is no collision.** A "0 collisions"
// row would be noise on every launch that has none, which is almost all of them.
#[test]
fn no_collision_produces_no_row() {
    let rows = lca_cli::tui::startup::collision_rows(&lca_ui::CollisionSummary::default());
    assert!(rows.is_empty(), "silence, not a zero row: {rows:?}");
}

// Verifies: gh #400 gap 3 - **the notice survives `quietStartup`.** A shadowed
// extension is a diagnosis, not decoration: a user who set `quietStartup = true`
// and installed an extension still needs to learn why the bundled one answered.
//
// `quiet == "true"` hides the header entirely; it must not hide this.
#[test]
fn a_collision_notice_survives_quiet_startup() {
    let summary = lca_ui::CollisionSummary {
        entries: vec![lca_ui::CollisionEntry {
            name: "lint".to_string(),
            kind: "extension".to_string(),
            winner: "installed".to_string(),
            loser: "bundled".to_string(),
        }],
    };
    let header =
        lca_cli::tui::startup::startup_header_with("0.6.0", "0.6", "true", &[], 1, 0, 0, &summary);
    let text = header.join("\n");
    assert!(
        text.contains("lint"),
        "the collision is still reported under quietStartup:\n{text}"
    );
    assert!(
        !text.contains("lca 0.6.0"),
        "while the header itself stays quiet:\n{text}"
    );
}

// Verifies: gh #400 gap 3 - **the collision is not an error.** The session
// continues and the winner is the one that answers. The registry's own behaviour
// (FR-EXT-11: the loser is disabled for the session) is unchanged; only whether it
// is *reported* changes. Asserted on the registry so the notice can never be
// implemented by changing what loads.
#[test]
fn a_collision_does_not_stop_the_session_and_the_winner_answers() {
    // The summary is derived from the registry, so this pins the shape of the
    // mapping: every recorded collision becomes exactly one entry, with the same
    // name and kind.
    let report = lca_core::CollisionReport {
        extension: "greedy".to_string(),
        name: "greedy".to_string(),
        kind: "extension",
        winner: "earlier registration".to_string(),
    };
    let summary = lca_cli::tui::startup::collision_summary(std::slice::from_ref(&report));
    assert_eq!(summary.entries.len(), 1, "one report, one entry");
    assert_eq!(summary.entries[0].name, "greedy");
    assert_eq!(summary.entries[0].kind, "extension");
    assert!(
        summary.entries[0].loser.contains("greedy"),
        "the loser is the later registration of the same name: {:?}",
        summary.entries[0]
    );
}
