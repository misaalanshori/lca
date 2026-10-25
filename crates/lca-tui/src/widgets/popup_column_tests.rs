//! Gh #386 gap 1a: the autocomplete popup's description column.
//!
//! Two defects, one seam. Rows were formatted `{marker}{label}{"  "+desc}`, so
//! descriptions began at a different column on every row; and there was no
//! shared width rule at all.
//!
//! The rule is pi's (`select-list.ts`'s `getPrimaryColumnWidth` +
//! `renderItem`): one column measured from the widest *visible label*,
//! `clamp(widest + 2, 12, 32)`, and the description column appears only when
//! `width > 40` **and** the remainder exceeds 10 - because on a narrow
//! terminal a description column that steals the label's space is worse than
//! no column at all.
//!
//! Split from `editor_tests.rs` for the workspace's 1,200-line ceiling
//! (gate 11); the popup's navigation and accept rows stay there.

use super::*;
use crate::widgets::autocomplete::{AutocompleteItem, Origin, OriginScope, Suggestions};

/// Suggestions from a fixed row list, with the first row selected.
fn suggestions(items: Vec<AutocompleteItem>) -> Suggestions {
    Suggestions {
        items,
        prefix: "/".to_string(),
    }
}

/// A plain built-in row.
fn row(label: &str, description: Option<&str>) -> AutocompleteItem {
    AutocompleteItem {
        value: label.to_string(),
        label: label.to_string(),
        description: description.map(str::to_string),
        origin: Origin::Builtin,
        dim: false,
    }
}

/// An editor showing `items`.
fn editor_with(items: Vec<AutocompleteItem>) -> Editor {
    let mut editor = Editor::new();
    editor.set_suggestions(suggestions(items));
    editor
}

/// The cell column at which `needle` starts on `line`, or `None`.
fn column_of(line: &str, needle: &str) -> Option<usize> {
    let stripped = crate::engine::text::strip_terminal_sequences(line);
    let byte = stripped.find(needle)?;
    // Convert the byte offset to a *cell* column: a CJK glyph before the
    // needle occupies two cells, and this helper is what proves the
    // implementation did not forget that.
    Some(crate::engine::text::visible_width(&stripped[..byte]))
}

// ---------------------------------------------------------------------------
// The column (the zigzag defect)
// ---------------------------------------------------------------------------

// The issue's own shape: labels of different lengths, descriptions that must
// line up. `▸ ` is 2 cells, so with a 20-cell label column every description
// starts at cell 22.
#[test]
fn descriptions_start_at_the_same_column_across_rows() {
    let editor = editor_with(vec![
        row("/help", Some("List every command")),
        row("/model", Some("Pick a model")),
        row("/scoped-models", Some("Choose which models cycle")),
        row("/mcp", Some("Manage MCP servers")),
    ]);
    let lines = editor.render_popup(80);
    let columns: Vec<Option<usize>> = lines
        .iter()
        .map(|l| column_of(l, "List every command"))
        .collect();
    // Only the first row carries that exact text; assert the general rule
    // instead: every description starts at one column.
    let _ = columns;
    let starts: Vec<usize> = lines
        .iter()
        .map(|line| {
            // The description is the run after the padded label; find it by
            // the known text of each row.
            [
                "List every command",
                "Pick a model",
                "Choose which models cycle",
                "Manage MCP servers",
            ]
            .iter()
            .find_map(|needle| column_of(line, needle))
            .expect("every row has its description")
        })
        .collect();
    assert!(
        starts.windows(2).all(|w| w[0] == w[1]),
        "descriptions share one column (got {starts:?}):\n{}",
        lines.join("\n")
    );
}

// The column is `max(label + 2)` over the *visible* labels, plus the marker -
// clamped. A tiny set clamps up to the 12-cell minimum.
#[test]
fn a_tiny_label_set_clamps_up_to_the_minimum_column() {
    let editor = editor_with(vec![row("/a", Some("first")), row("/b", Some("second"))]);
    let lines = editor.render_popup(80);
    // `/a` is 2 cells + 2 gap = 4, clamped up to 12; marker is 2, so the
    // description starts at cell 14.
    let column = column_of(&lines[0], "first").expect("the description is painted");
    assert_eq!(
        column,
        14,
        "a narrow label set clamps up to the 12-cell column (got {column}):\n{}",
        lines.join("\n")
    );
}

// And a huge label set clamps down to the 32-cell maximum.
#[test]
fn a_huge_label_set_clamps_down_to_the_maximum_column() {
    let long = format!("/{}", "x".repeat(60));
    let editor = editor_with(vec![
        row(&long, Some("described")),
        row("/s", Some("short")),
    ]);
    let lines = editor.render_popup(80);
    let column = column_of(&lines[0], "described").expect("the description is painted");
    // 32-cell column + the 2-cell marker.
    assert_eq!(
        column,
        34,
        "a wide label set clamps down to the 32-cell column (got {column}):\n{}",
        lines.join("\n")
    );
    // The label itself is truncated into its column, never allowed to push
    // the row past the pane.
    for line in &lines {
        assert!(
            crate::engine::text::visible_width(line) <= 80,
            "a row never exceeds the pane: {line:?}"
        );
    }
}

// Threshold one: a pane at 40 cells has no description column.
#[test]
fn a_narrow_pane_has_no_description_column() {
    let editor = editor_with(vec![
        row("/help", Some("List every command")),
        row("/mcp", Some("Manage MCP")),
    ]);
    for width in [20u16, 30, 40] {
        let lines = editor.render_popup(width);
        assert!(
            !lines[0].contains("List every command"),
            "at {width} cells the description column is dropped:\n{}",
            lines.join("\n")
        );
        for line in &lines {
            assert!(
                crate::engine::text::visible_width(line) <= width as usize,
                "a row fits the pane at {width}: {line:?}"
            );
        }
    }
    // And one cell past the threshold it appears, so the boundary is tested
    // from both sides.
    let lines = editor.render_popup(41);
    assert!(
        lines[0].contains("List every command"),
        "at 41 cells the column exists:\n{}",
        lines.join("\n")
    );
}

// Threshold two: the pane is wider than 40, but the solved column leaves so
// little room that a description would be useless - so it is dropped. This is
// the threshold a wide label set trips on a pane a narrow one would keep.
//
// The arithmetic, so the rows below are not guesses. A maximum-width label set
// solves its column to 32, the marker is 2, and the column is capped to
// `width - (marker + 4)`. At 46 the cap bites (32 -> 32 is still 32, so
// `46 - (2 + 32 + 2) = 10`, which is not "greater than" 10 -> dropped); at 47
// the remainder is 11 and the column appears.
#[test]
fn a_narrow_remainder_has_no_description_column() {
    let long = format!("/{}", "x".repeat(60));
    for width in [41u16, 44, 46] {
        let editor = editor_with(vec![
            row(&long, Some("described")),
            row("/s", Some("short")),
        ]);
        let lines = editor.render_popup(width);
        assert!(
            !lines[0].contains("described"),
            "at {width} cells the remainder does not exceed 10, so no description column:\n{}",
            lines.join("\n")
        );
        for line in &lines {
            assert!(
                crate::engine::text::visible_width(line) <= width as usize,
                "a row fits the pane at {width}: {line:?}"
            );
        }
    }
    // One cell past it the column returns, so the boundary is tested from both
    // sides rather than only from below.
    let editor = editor_with(vec![
        row(&long, Some("described")),
        row("/s", Some("short")),
    ]);
    let lines = editor.render_popup(47);
    assert!(
        lines[0].contains("described"),
        "at 47 cells the remainder clears the threshold:\n{}",
        lines.join("\n")
    );
}

// **Measurement in cells, not bytes.** A CJK label must not shift the column:
// four `日` glyphs are 12 bytes and 8 cells.
#[test]
fn a_cjk_label_does_not_shift_the_column() {
    let editor = editor_with(vec![
        row("/日本", Some("cjk label")),
        row("/help", Some("ascii label")),
        row("/model", Some("another")),
    ]);
    let lines = editor.render_popup(80);
    let starts: Vec<usize> = ["cjk label", "ascii label", "another"]
        .iter()
        .zip(lines.iter())
        .map(|(needle, line)| column_of(line, needle).expect("the description is painted"))
        .collect();
    assert!(
        starts.windows(2).all(|w| w[0] == w[1]),
        "a CJK label measures in cells, so the column does not move (got {starts:?}):\n{}",
        lines.join("\n")
    );
    // The widest label is `/model` (6 cells) + 2 = 8, clamped up to 12; plus
    // the 2-cell marker.
    assert_eq!(starts[0], 14, "the 12-cell minimum, counted in cells");
}

// A **long description is truncated** within the row, and the label is what
// survives - the label is what the user is choosing between.
#[test]
fn a_long_description_is_truncated_and_the_label_survives() {
    let long = "x".repeat(200);
    let editor = editor_with(vec![
        row("/model", Some(&long)),
        row("/help", Some("short")),
    ]);
    let lines = editor.render_popup(60);
    for line in &lines {
        assert!(
            crate::engine::text::visible_width(line) <= 60,
            "no row exceeds the pane: {line:?}"
        );
    }
    assert!(
        lines[0].contains("/model"),
        "the label is intact even though the description was cut: {:?}",
        lines[0]
    );
    // And the cut is visible rather than silently dropped mid-word.
    assert!(
        lines[0].contains('…'),
        "a truncated description is marked: {:?}",
        lines[0]
    );
}

// A description carrying newlines is normalized to one line, so a multi-line
// description cannot break the column.
#[test]
fn a_multiline_description_is_flattened_to_one_line() {
    let editor = editor_with(vec![
        row("/model", Some("first line\nsecond line")),
        row("/help", Some("x")),
    ]);
    let lines = editor.render_popup(80);
    assert_eq!(
        lines.len(),
        2,
        "one row per item, not one per line:\n{}",
        lines.join("\n")
    );
    assert!(
        lines[0].contains("first line second line"),
        "the newline became a space: {:?}",
        lines[0]
    );
}

// ---------------------------------------------------------------------------
// Provenance (the second half)
// ---------------------------------------------------------------------------

// Every origin kind is spelled by the renderer, and the four are distinct.
#[test]
fn every_origin_kind_is_visible_and_distinct() {
    let origins = [
        Origin::Builtin,
        Origin::Extension("github-issues".to_string()),
        Origin::Skill,
        Origin::PromptTemplate {
            scope: OriginScope::Project,
        },
    ];
    let mut labels = Vec::new();
    for origin in origins {
        let mut item = row("/thing", Some("same description"));
        item.origin = origin.clone();
        let editor = editor_with(vec![item]);
        let line = editor.render_popup(80).into_iter().next().expect("a row");
        labels.push((origin, line));
    }
    // The three non-built-in kinds each carry a tag; the built-in one does not
    // (a tag on every row is noise - the brief's rule 6 keeps the column
    // useful for whatever actually has provenance).
    for (origin, line) in &labels {
        match origin {
            Origin::Builtin => assert!(
                !line.contains('['),
                "a built-in row carries no provenance tag: {line:?}"
            ),
            _ => assert!(line.contains('['), "{origin:?} is tagged: {line:?}"),
        }
    }
}

// **The row that makes the second half real**: two same-named commands from
// different scopes must be tellable apart.
#[test]
fn two_same_named_commands_from_different_scopes_are_distinguishable() {
    let mut project = row("/deslop", Some("Clean the prose"));
    project.origin = Origin::PromptTemplate {
        scope: OriginScope::Project,
    };
    let mut user = row("/deslop", Some("Clean the prose"));
    user.origin = Origin::PromptTemplate {
        scope: OriginScope::User,
    };
    let editor = editor_with(vec![project, user]);
    let lines = editor.render_popup(100);
    assert_ne!(
        lines[0],
        lines[1],
        "the two `/deslop` rows differ:\n{}",
        lines.join("\n")
    );
    assert!(
        lines[0].contains("project") && lines[1].contains("user"),
        "each names its scope:\n{}",
        lines.join("\n")
    );
}

// A skill's provenance rides in the description column *with* its own
// description, not instead of it.
#[test]
fn a_skill_keeps_its_description_alongside_its_provenance() {
    let mut item = row(
        "/skill:deploy-cloudflare",
        Some("Deploy to Cloudflare Workers"),
    );
    item.origin = Origin::Skill;
    let editor = editor_with(vec![item]);
    let line = editor.render_popup(100).into_iter().next().expect("a row");
    assert!(
        line.contains("Deploy to Cloudflare Workers"),
        "the description survives: {line:?}"
    );
    assert!(
        line.contains("skill"),
        "and the provenance is there: {line:?}"
    );
}

// An extension row names the extension, so two extensions offering the same
// command name cannot be confused.
#[test]
fn two_extensions_offering_the_same_name_are_distinguishable() {
    let mut a = row("/review", Some("Review the diff"));
    a.origin = Origin::Extension("github-issues".to_string());
    let mut b = row("/review", Some("Review the diff"));
    b.origin = Origin::Extension("gitlab-mr".to_string());
    let editor = editor_with(vec![a, b]);
    let lines = editor.render_popup(100);
    assert!(
        lines[0].contains("github-issues") && lines[1].contains("gitlab-mr"),
        "each row names its extension:\n{}",
        lines.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Styling and the existing contract
// ---------------------------------------------------------------------------

// Selected and unselected rows are styled uniformly: the popup renders plain
// text and the *caller* styles a whole selected row, so a selected row cannot
// have a dim description beside a styled label.
#[test]
fn the_popup_renders_uniformly_styled_rows() {
    let editor = editor_with(vec![row("/help", Some("one")), row("/model", Some("two"))]);
    let lines = editor.render_popup(80);
    for line in &lines {
        assert!(
            !line.contains('\x1b'),
            "the popup emits no escapes of its own; the caller styles whole rows: {line:?}"
        );
    }
}

// The existing navigation and accept rows keep passing (they live in
// `editor_tests.rs`); this pins that the popup still reports one row per item
// with the marker on the selected one.
#[test]
fn the_selected_row_still_carries_its_marker() {
    let editor = editor_with(vec![row("/help", Some("one")), row("/model", Some("two"))]);
    let lines = editor.render_popup(80);
    assert!(
        lines[0].starts_with("▸ "),
        "the first row is selected: {lines:?}"
    );
    assert!(
        lines[1].starts_with("  "),
        "and the second is not: {lines:?}"
    );
    assert_eq!(editor.suggestion_index(), 0);
}

// A row with no description still occupies the label column cleanly, so a
// mixed list does not leave ragged spaces.
#[test]
fn a_row_without_a_description_has_no_trailing_padding() {
    let editor = editor_with(vec![
        row("/help", None),
        row("/model", Some("Pick a model")),
    ]);
    let lines = editor.render_popup(80);
    assert_eq!(
        lines[0].trim_end(),
        lines[0],
        "no trailing spaces on a description-less row: {:?}",
        lines[0]
    );
}
