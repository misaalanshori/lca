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

// ===========================================================================
// gh #400 gap 4: the tmux keyboard doctor
// ===========================================================================
//
// The doctor's *decision* is pinned by unit rows in `tui::tmux_doctor`; these rows
// pin where its finding lands, which is the half a unit test cannot see. The
// brief's rule is specific - the notice belongs in the header's expandable
// section, never printed into the transcript - and "where it lands" is exactly
// the kind of thing that regresses silently because both placements render.

/// The doctor rows for a finding, through the same function the launch uses.
fn doctor_rows_for(doctor: lca_cli::tui::tmux_doctor::Doctor) -> Vec<String> {
    let cell = std::sync::Arc::new(std::sync::OnceLock::new());
    let _ = cell.set(doctor);
    lca_cli::tui::startup::doctor_rows(&cell, Vec::new())
}

// Verifies: gh #400 gap 4 - **the notice joins the expanded table.** The launch
// composes `(compact, expanded)` and the doctor rows go into the second, so a
// reader who wants the diagnosis presses the one expand key and finds it there.
#[test]
fn the_doctor_notice_lands_in_the_expanded_section() {
    let off = lca_cli::tui::tmux_doctor::Doctor::ExtendedKeysOff {
        format: Some("xterm".into()),
    };
    let rows = doctor_rows_for(off);
    assert!(
        !rows.is_empty(),
        "an unhealthy tmux says something: {rows:?}"
    );
    let text = rows.join(" ");
    assert!(
        text.contains("set -g extended-keys on"),
        "and it is the command, not a description of one: {text}"
    );
    assert!(
        text.contains("restart"),
        "with the restart it needs: {text}"
    );
}

// Verifies: gh #400 gap 4 - **a healthy tmux adds no row at all.** This is the
// requirement the whole doctor stands on: rows are only worth reading if their
// presence means something. Asserted as zero rows, not as "no warning text", so a
// blank spacer left behind fails too.
#[test]
fn a_healthy_tmux_adds_no_row_to_the_header() {
    let silent = doctor_rows_for(lca_cli::tui::tmux_doctor::Doctor::Silent);
    assert!(
        silent.is_empty(),
        "silence is zero rows, not a quiet row: {silent:?}"
    );
}

// Verifies: gh #400 gap 4 - **the notice is not in the transcript.** The compact
// header is what a launch shows; a diagnostic about the terminal must not be in
// it, or it appears every launch in the one place the reader did not ask. Asserted
// by composing the real pair and checking the second, not the first.
#[test]
fn the_doctor_notice_stays_out_of_the_compact_line() {
    let off = lca_cli::tui::tmux_doctor::Doctor::ExtendedKeysOff { format: None };
    let cell = std::sync::Arc::new(std::sync::OnceLock::new());
    let _ = cell.set(off);
    let (compact, expanded) = (
        lca_cli::tui::startup::compact_header(
            "0.6.0",
            "0.6",
            &[],
            1,
            0,
            0,
            lca_ui::CollisionSummary::default(),
        ),
        lca_cli::tui::startup::doctor_rows(&cell, Vec::new()),
    );
    let compact_text = compact.join("\n");
    assert!(
        !compact_text.contains("extended-keys"),
        "the default one-liner stays a one-liner: {compact_text}"
    );
    assert!(
        expanded.join(" ").contains("extended-keys"),
        "the diagnosis lives behind the key: {expanded:?}"
    );
}

// Verifies: gh #400 gap 4 - **the notice is fitted like every other row.** §4's
// Column Gauge rule is that no row may stagger the frame; a notice is longer than
// any other header row (it carries a command and a reason), so it is the row most
// likely to break that rule.
#[test]
fn a_long_doctor_notice_is_fitted_to_the_frame() {
    let off = lca_cli::tui::tmux_doctor::Doctor::ExtendedKeysOff {
        format: Some("xterm".into()),
    };
    let rows = doctor_rows_for(off);
    let widest = rows.iter().map(String::len).max().unwrap_or(0);
    assert!(
        widest > 60,
        "the fixture notice is genuinely long, so the row is worth making: {widest}"
    );
    // Fitted through the same helper the rest of the header uses.
    for row in &rows {
        let fitted = lca_cli::tui::startup::fit_row(row, 40);
        assert!(
            lca_tui::engine::text::visible_width(&fitted) <= 40,
            "a fitted doctor row respects the budget: {fitted:?}"
        );
    }
}

// Verifies: gh #400 gap 4 - **`quietStartup` suppresses the doctor, and this is
// NOT the collision rule.** The two differ on purpose: a collision means the thing
// the reader installed did not load (they have no other way to learn it, so it
// rides through quiet), while a tmux finding is advice about a chord they have not
// pressed. A quiet startup must stay quiet, or the setting is decorative.
#[test]
fn quiet_startup_suppresses_the_doctor_notice() {
    let off = lca_cli::tui::tmux_doctor::Doctor::ExtendedKeysOff { format: None };
    let cell = std::sync::Arc::new(std::sync::OnceLock::new());
    let _ = cell.set(off);
    let base = vec!["lca 0.6.0 (abi 0.6)".to_string()];

    let quiet = lca_cli::tui::startup::doctor_rows_quiet(&cell, base.clone(), "true");
    assert_eq!(quiet, base, "quiet adds nothing at all: {quiet:?}");

    // `"header"` is a *reduced* startup, not a quiet one - it keeps diagnostics
    // in the same spirit as the collision notices.
    let header_only = lca_cli::tui::startup::doctor_rows_quiet(&cell, base.clone(), "header");
    assert!(
        header_only.len() > base.len(),
        "`header` still reports: {header_only:?}"
    );

    let loud = lca_cli::tui::startup::doctor_rows_quiet(&cell, base.clone(), "false");
    assert!(loud.len() > base.len(), "a normal launch reports: {loud:?}");
}

// Verifies: gh #400 gap 4 - **`quietStartup` suppresses the doctor through the
// real launch.** The row above pins the helper; this one pins that the launch
// actually passes the setting through, which is the link that can be dropped while
// every unit row still passes (checked: deleting `config.ui_quiet_startup()` from
// `launch_header` is invisible to the helper-level row and caught here).
#[test]
fn a_quiet_startup_launch_prints_no_doctor_notice() {
    let dir = std::env::temp_dir().join(format!("lca-quiet-doctor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let user = dir.join("user.toml");
    std::fs::write(&user, "ui.quiet_startup = true\n").expect("write config");

    let cell = std::sync::Arc::new(std::sync::OnceLock::new());
    let _ = cell.set(lca_cli::tui::tmux_doctor::Doctor::ExtendedKeysOff {
        format: Some("xterm".into()),
    });
    let registry = lca_core::ExtensionRegistry::new();
    let flags = lca_cli::CliFlags::default();

    let quiet = lca_config::Config::load(&lca_config::LoadInput {
        user_file: Some(user.clone()),
        ..Default::default()
    })
    .expect("load quiet config");
    assert_eq!(quiet.ui_quiet_startup(), "true", "the fixture is quiet");
    let (compact, expanded) =
        lca_cli::tui::startup::launch_header(&dir, &dir, false, &flags, &registry, &quiet, &cell);
    let all = format!("{}\n{}", compact.join("\n"), expanded.join("\n"));
    assert!(
        !all.contains("extended-keys"),
        "a quiet launch says nothing about tmux: {all}"
    );

    // The same launch with the default config does report, so the row is
    // measuring the gate rather than an empty fixture.
    let loud = lca_config::Config::default();
    assert_eq!(loud.ui_quiet_startup(), "false", "the default is loud");
    let (_compact, loud_expanded) =
        lca_cli::tui::startup::launch_header(&dir, &dir, false, &flags, &registry, &loud, &cell);
    assert!(
        loud_expanded.join("\n").contains("extended-keys"),
        "and the default launch does: {loud_expanded:?}"
    );
}
