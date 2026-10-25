//! Gh #386 gap 1: the autocomplete popup's ragged description column, and rows
//! that carried no provenance.
//!
//! Two defects in one seam:
//!
//! 1. **Zigzag.** Rows were formatted `{marker}{label}{"  "+desc}`, so every
//!    description began wherever its own label happened to end.
//! 2. **No provenance.** `/deslop`, `/debug-models` and `/skill:deploy-cloudflare`
//!    looked interchangeable, and two same-named commands from different scopes
//!    were indistinguishable.
//!
//! The fix is pi's `select-list.ts` rule: one column for the whole list,
//! `clamp(widest visible label + 2, 12, 32)`, with the description column
//! appearing only when the pane is wider than 40 cells **and** more than 10
//! cells remain for it. Provenance became a typed `Origin` - required on the
//! item, so the compiler lists every constructor.
//!
//! Gaps 2 (hover fighting centered windowing) and 3 (`Space` in `/settings`)
//! are out of scope.
//!
//! Verifies: FR-UI-9 (defect gh #386 gap 1). The unit rows live in
//! `crates/lca-tui/src/widgets/popup_column_tests.rs`.

use lca_tui::engine::text::{strip_terminal_sequences, visible_width};
use lca_tui::widgets::autocomplete::{AutocompleteItem, Origin, OriginScope, Suggestions};
use lca_tui::widgets::editor::Editor;
use lca_tui::widgets::editor::popup_layout::{
    MAX_PRIMARY_COLUMN_WIDTH, MIN_DESCRIPTION_WIDTH, MIN_PRIMARY_COLUMN_WIDTH,
    MIN_TWO_COLUMN_WIDTH, PRIMARY_COLUMN_GAP,
};

/// A row with the given label and description, built in.
fn row(label: &str, description: Option<&str>) -> AutocompleteItem {
    AutocompleteItem {
        value: label.to_string(),
        label: label.to_string(),
        description: description.map(str::to_string),
        origin: Origin::Builtin,
        dim: false,
    }
}

/// An editor whose popup shows `items`.
fn popup(items: Vec<AutocompleteItem>, width: u16) -> Vec<String> {
    let mut editor = Editor::new();
    editor.set_suggestions(Suggestions {
        items,
        prefix: "/".to_string(),
    });
    editor.render_popup(width)
}

/// The cell column at which `needle` begins on `line`.
fn column_of(line: &str, needle: &str) -> usize {
    let stripped = strip_terminal_sequences(line);
    let at = stripped.find(needle).expect("the needle is on the line");
    visible_width(&stripped[..at])
}

#[test]
fn every_description_lands_in_one_column() {
    let lines = popup(
        vec![
            row("/help", Some("List every command")),
            row("/model", Some("Pick a model")),
            row("/scoped-models", Some("Choose which models cycle")),
            row("/mcp", Some("Manage MCP servers")),
        ],
        80,
    );
    let columns: Vec<usize> = [
        "List every command",
        "Pick a model",
        "Choose which models cycle",
        "Manage MCP servers",
    ]
    .iter()
    .zip(&lines)
    .map(|(needle, line)| column_of(line, needle))
    .collect();
    assert!(
        columns.windows(2).all(|w| w[0] == w[1]),
        "all four rows share one description column (got {columns:?}):\n{}",
        lines.join("\n")
    );
}

#[test]
fn the_column_is_clamped_to_pis_bounds() {
    // A tiny label set clamps UP to the minimum: `/a` is 2 cells, +2 gap = 4.
    let lines = popup(
        vec![row("/a", Some("first")), row("/b", Some("second"))],
        80,
    );
    assert_eq!(
        column_of(&lines[0], "first"),
        2 + MIN_PRIMARY_COLUMN_WIDTH,
        "a narrow label set clamps up to {MIN_PRIMARY_COLUMN_WIDTH} cells"
    );

    // A huge one clamps DOWN to the maximum.
    let long = format!("/{}", "x".repeat(60));
    let lines = popup(vec![row(&long, Some("described"))], 120);
    assert_eq!(
        column_of(&lines[0], "described"),
        2 + MAX_PRIMARY_COLUMN_WIDTH,
        "a wide label set clamps down to {MAX_PRIMARY_COLUMN_WIDTH} cells"
    );
}

/// Threshold one: the pane must be **wider** than `MIN_TWO_COLUMN_WIDTH`.
#[test]
fn a_pane_at_the_width_threshold_has_no_description_column() {
    let items = || {
        vec![
            row("/help", Some("List every command")),
            row("/mcp", Some("MCP")),
        ]
    };
    let at = popup(items(), MIN_TWO_COLUMN_WIDTH as u16);
    assert!(
        !at[0].contains("List every command"),
        "at exactly {MIN_TWO_COLUMN_WIDTH} cells there is no description column:\n{}",
        at.join("\n")
    );
    let past = popup(items(), (MIN_TWO_COLUMN_WIDTH + 1) as u16);
    assert!(
        past[0].contains("List every command"),
        "one cell wider, the column exists:\n{}",
        past.join("\n")
    );
}

/// Threshold two: the remainder must **exceed** `MIN_DESCRIPTION_WIDTH`.
#[test]
fn a_narrow_remainder_at_the_threshold_has_no_description_column() {
    // A maximum-width label set narrows the remainder even on a wide pane.
    let long = format!("/{}", "x".repeat(60));
    let items = || vec![row(&long, Some("described"))];
    // `w - (2 marker + 32 column + 2 margin)` must exceed 10, so 47 is the
    // first width that keeps the column.
    let boundary = 2 + MAX_PRIMARY_COLUMN_WIDTH + 2 + MIN_DESCRIPTION_WIDTH;
    let at = popup(items(), boundary as u16);
    assert!(
        !at[0].contains("described"),
        "a remainder of exactly {MIN_DESCRIPTION_WIDTH} cells is not enough:\n{}",
        at.join("\n")
    );
    let past = popup(items(), (boundary + 1) as u16);
    assert!(
        past[0].contains("described"),
        "one cell of remainder more, it appears:\n{}",
        past.join("\n")
    );
}

/// Cells, not bytes: a CJK label must not move the column.
#[test]
fn a_cjk_label_does_not_move_the_column() {
    let lines = popup(
        vec![
            row("/日本", Some("cjk")),
            row("/help", Some("ascii")),
            row("/model", Some("another")),
        ],
        80,
    );
    let columns: Vec<usize> = ["cjk", "ascii", "another"]
        .iter()
        .zip(&lines)
        .map(|(needle, line)| column_of(line, needle))
        .collect();
    assert!(
        columns.windows(2).all(|w| w[0] == w[1]),
        "measured in cells, the column is shared (got {columns:?}):\n{}",
        lines.join("\n")
    );
}

/// A long description is truncated, and the *label* is what survives.
#[test]
fn a_long_description_is_cut_and_the_label_survives() {
    let lines = popup(vec![row("/model", Some(&"x".repeat(300)))], 60);
    assert!(
        visible_width(&lines[0]) <= 60,
        "the row fits the pane: {:?}",
        lines[0]
    );
    assert!(
        lines[0].contains("/model"),
        "the label is intact: {:?}",
        lines[0]
    );
    assert!(lines[0].contains('…'), "the cut is marked: {:?}", lines[0]);
}

#[test]
fn every_origin_kind_is_spelled_by_the_renderer() {
    // A built-in row carries no tag; the three others each do, and they differ.
    let mut tagged = Vec::new();
    for origin in [
        Origin::Extension("github-issues".to_string()),
        Origin::Skill,
        Origin::PromptTemplate {
            scope: OriginScope::Project,
        },
    ] {
        let tag = origin
            .tag()
            .unwrap_or_else(|| panic!("{origin:?} carries a tag"));
        let mut item = row("/thing", Some("described"));
        item.origin = origin;
        let line = popup(vec![item], 100).into_iter().next().expect("a row");
        assert!(
            line.contains(&tag),
            "{tag:?} is rendered on the row: {line:?}"
        );
        tagged.push(tag);
    }
    assert_eq!(
        tagged,
        vec![
            "extension: github-issues".to_string(),
            "skill".to_string(),
            "prompt: project".to_string()
        ]
    );
    assert_eq!(Origin::Builtin.tag(), None, "a built-in row is untagged");
}

/// **The row that makes the second half real**: two same-named commands from
/// different scopes must be tellable apart.
#[test]
fn two_same_named_commands_from_different_scopes_differ() {
    let mut project = row("/deslop", Some("Clean the prose"));
    project.origin = Origin::PromptTemplate {
        scope: OriginScope::Project,
    };
    let mut user = row("/deslop", Some("Clean the prose"));
    user.origin = Origin::PromptTemplate {
        scope: OriginScope::User,
    };
    let lines = popup(vec![project, user], 100);
    assert_ne!(
        lines[0],
        lines[1],
        "the two `/deslop` rows differ:\n{}",
        lines.join("\n")
    );
    assert!(
        lines[0].contains("project") && lines[1].contains("user"),
        "each names its own scope:\n{}",
        lines.join("\n")
    );
}

/// A row that already has a description keeps it; the tag rides after it.
#[test]
fn a_description_and_its_provenance_share_the_column() {
    let mut item = row(
        "/skill:deploy-cloudflare",
        Some("Deploy to Cloudflare Workers"),
    );
    item.origin = Origin::Skill;
    let line = popup(vec![item], 100).into_iter().next().expect("a row");
    assert!(
        line.contains("Deploy to Cloudflare Workers"),
        "the description is kept, not replaced by the tag: {line:?}"
    );
    assert!(line.contains("skill"), "and the tag is there: {line:?}");
    // The tag follows the description in the same column, rather than opening a
    // third one.
    let desc_at = line.find("Deploy to Cloudflare Workers").expect("present");
    let tag_at = line.find("[skill]").expect("present");
    assert!(
        tag_at > desc_at,
        "the tag follows the description: {line:?}"
    );
}

/// The popup still renders plain text, so the caller can style a whole
/// selected row (pi's `selectedText`) instead of a label separately.
#[test]
fn rows_carry_no_escapes_of_their_own() {
    let lines = popup(
        vec![row("/help", Some("one")), row("/model", Some("two"))],
        80,
    );
    for line in &lines {
        assert!(
            !line.contains('\x1b'),
            "the popup emits no escapes: {line:?}"
        );
    }
}

/// The named constants are the rule, not magic numbers in the renderer.
#[test]
fn the_column_rule_is_named() {
    assert_eq!(MIN_PRIMARY_COLUMN_WIDTH, 12);
    assert_eq!(MAX_PRIMARY_COLUMN_WIDTH, 32);
    assert_eq!(PRIMARY_COLUMN_GAP, 2);
    assert_eq!(MIN_TWO_COLUMN_WIDTH, 40);
    assert_eq!(MIN_DESCRIPTION_WIDTH, 10);
    const _: () = assert!(MIN_PRIMARY_COLUMN_WIDTH < MAX_PRIMARY_COLUMN_WIDTH);
}

/// The host supplies `slash_commands` as bare names, so `lca-ui` recovers the
/// provenance from the name shapes the host already produces. This pins that
/// classifier - it is the seam that makes the `/` menu's rows typed without
/// widening a host-facing `Vec<String>`.
///
/// Recovered rather than widened because `lca-cli` (which builds that list) is
/// outside this cycle's file boundary; the report records it as the deviation.
#[test]
fn the_command_name_recovers_its_provenance() {
    use lca_ui::autocomplete_provider::origin_for_command;
    assert_eq!(origin_for_command("help"), Origin::Builtin);
    assert_eq!(origin_for_command("model"), Origin::Builtin);
    assert_eq!(origin_for_command("skill:deploy-cloudflare"), Origin::Skill);
    assert_eq!(
        origin_for_command("openai-compatible.login"),
        Origin::Extension("openai-compatible".to_string())
    );
    assert_eq!(
        origin_for_command("openai-compatible.usage"),
        Origin::Extension("openai-compatible".to_string()),
        "every identity command names its extension, not just `.login`"
    );
    // A built-in whose name merely *contains* a dot is not an extension.
    assert_eq!(origin_for_command("scoped-models"), Origin::Builtin);
    assert_eq!(origin_for_command(""), Origin::Builtin);
}

/// The classifier's output reaches the popup through `SlashCommand::origin`, so
/// a command row is typed without the renderer guessing.
#[test]
fn a_command_row_carries_its_slash_command_origin() {
    use lca_tui::widgets::autocomplete::{
        AutocompleteProvider, CombinedAutocompleteProvider, SlashCommand,
    };

    let provider = CombinedAutocompleteProvider::new(
        vec![
            SlashCommand {
                name: "help".to_string(),
                description: None,
                argument_hint: None,
                argument_completions: None,
                origin: Origin::Builtin,
            },
            SlashCommand {
                name: "skill:deploy".to_string(),
                description: None,
                argument_hint: None,
                argument_completions: None,
                origin: Origin::Skill,
            },
        ],
        std::env::temp_dir(),
    );
    let suggestions = provider
        .get_suggestions("/", false)
        .expect("the bare slash lists commands");
    let skill = suggestions
        .items
        .iter()
        .find(|item| item.label.starts_with("/skill:"))
        .expect("the skill row is offered");
    assert_eq!(skill.origin, Origin::Skill, "the row is a skill");
    let help = suggestions
        .items
        .iter()
        .find(|item| item.label == "/help")
        .expect("the built-in row is offered");
    assert_eq!(help.origin, Origin::Builtin);
}
