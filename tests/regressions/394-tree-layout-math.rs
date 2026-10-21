//! GitHub #394: the `/tree` navigator's layout math broke on ordinary
//! sessions. `TreePicker::rebuild` set `depth = chain.len()` - the count of
//! *visible ancestors* - and `paint_rows` emitted one three-column gutter
//! per ancestor. A 40-turn linear session (80 rows, one child each) is
//! therefore indented 160 columns deep by its last row: the whole
//! conversation marches diagonally off the right edge of any real
//! terminal, and what remains on screen is connector columns and no text.
//!
//! The fix is pi's `tree-selector.ts` rule: indent grows only where a
//! visible parent has more than one visible child, plus one grouping level
//! beneath a fresh fork. A linear chain stays at column 0 however long it
//! is. Three further rules ride along - the live trunk sorts first,
//! filtering re-anchors the cursor to the nearest visible ancestor, and
//! folds paint `⊞`/`⊟` in the connector cell with an anchor-aware pan.
//!
//! This row is the end-to-end guard: it builds a real 40-turn session (80
//! records) through the real store, converts it through the same mapping
//! the CLI hook uses, and asserts the frame. On the pre-fix tree every row
//! past the fifth carried gutter columns instead of its text.
//!
//! Verifies: NFR-24 (a shipped defect's guard), GitHub issue #394,
//! gh #394 acceptance 1 (linear sessions stay at column 0).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{SessionStore, ViewMode};
use lca_ui::state::{TreeRow, TreeRowKind};

/// A scratch data directory, removed on drop.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> (Scratch, std::path::PathBuf) {
    // A counter, not just the pid: these rows run in parallel in one
    // process, and each `Scratch` removes its directory on drop.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let base =
        std::env::temp_dir().join(format!("lca-gh394-{name}-{}-{serial}", std::process::id()));
    let data = base.join("data");
    let project = base.join("project");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    (Scratch(base), data)
}

/// Map store rows to the interface's rows exactly as
/// `crates/lca-cli/src/tui/label_hooks.rs` does.
fn tree_rows(store: &SessionStore, session: &lca_session::Session) -> Vec<TreeRow> {
    store
        .entry_tree(session)
        .unwrap()
        .into_iter()
        .map(|row| {
            let kind = match row.kind {
                lca_session::EntryKind::User => TreeRowKind::User,
                lca_session::EntryKind::Assistant => TreeRowKind::Assistant,
                lca_session::EntryKind::Tool => TreeRowKind::Tool,
                lca_session::EntryKind::Summary => TreeRowKind::Summary,
                lca_session::EntryKind::Compaction => TreeRowKind::Compaction,
                lca_session::EntryKind::Custom => TreeRowKind::Custom,
                lca_session::EntryKind::CustomMessage => TreeRowKind::CustomMessage,
            };
            TreeRow {
                id: row.id,
                depth: row.depth,
                kind,
                text: row.text,
                call: row.call,
                label: row.label,
                live: row.live,
                live_leaf: row.live_leaf,
            }
        })
        .collect()
}

/// The 40-turn session the bug report describes: a user prompt and an
/// assistant reply per turn, each record a child of the last.
fn forty_turn_session() -> (Scratch, SessionStore, lca_session::Session) {
    let (scratch_dir, data) = scratch("linear");
    let store = SessionStore::new(data.clone());
    let project = data.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let session = store.create_session(&project, "linear forty").unwrap();
    for turn in 0..40 {
        store
            .append(
                &session,
                Record::User {
                    v: FORMAT_VERSION,
                    ts: turn * 2,
                    id: format!("u{turn}"),
                    parent: None,
                    content: format!("question {turn}"),
                    attachments: Vec::new(),
                    queue: None,
                },
            )
            .unwrap();
        store
            .append(
                &session,
                Record::Assistant {
                    v: FORMAT_VERSION,
                    ts: turn * 2 + 1,
                    id: format!("a{turn}"),
                    parent: None,
                    content: vec![lca_protocol::ContentBlock::Text {
                        text: format!("answer {turn}"),
                    }],
                    reasoning: None,
                    reasoning_signature: None,
                    provider_thinking_level: None,
                    model: None,
                    provider: None,
                    usage: None,
                },
            )
            .unwrap();
    }
    (scratch_dir, store, session)
}

/// The report's headline: every row of a long linear session sits at
/// column 0, with its text intact - no gutter, no connector, no march.
#[test]
fn a_forty_turn_linear_session_paints_every_row_at_column_zero() {
    let (_scratch, store, session) = forty_turn_session();
    let rows = tree_rows(&store, &session);
    assert_eq!(rows.len(), 80, "40 turns, two records each");
    let picker = lca_ui::state::tree_picker(rows, "default");
    let painted: Vec<String> = picker
        .paint_rows(&lca_ui::theme::Theme::plain(), usize::MAX)
        .iter()
        .map(|line| lca_tui::engine::text::strip_terminal_sequences(line))
        .collect();
    assert_eq!(painted.len(), 80, "every record is a row");
    for (turn, line) in painted.iter().enumerate() {
        let expected = if turn % 2 == 0 {
            format!("user: question {}", turn / 2)
        } else {
            format!("assistant: answer {}", turn / 2)
        };
        // The live chain marks each row with `• ` and closes with `◀`
        // (gh #248); neither is a gutter. Neither is anything else.
        let body = line
            .trim_start_matches("\u{2022} ")
            .trim_end_matches(" \u{25c0}");
        assert_eq!(
            body, &expected,
            "row {turn} must start at column 0 with no connector or gutter \
             (the pre-fix tree indented it {turn} columns deep): {line:?}"
        );
        assert!(
            !line.contains('\u{2502}') && !line.contains('\u{251c}') && !line.contains('\u{2514}'),
            "row {turn} carries no tree connector: {line:?}"
        );
    }
    let widest = painted
        .iter()
        .map(|line| lca_tui::engine::text::visible_width(line))
        .max()
        .unwrap_or(0);
    assert!(
        widest <= 24,
        "no row needs more than its own text: widest {widest}"
    );
}

/// The same session through a real 80-column frame: every painted row fits
/// inside the box, which is what "off the viewport" meant concretely.
#[test]
fn a_long_linear_session_fits_an_eighty_column_frame() {
    let (_scratch, store, session) = forty_turn_session();
    let rows = tree_rows(&store, &session);
    let picker = lca_ui::state::tree_picker(rows, "default");
    for width in [40usize, 80] {
        let painted = picker.paint_rows(&lca_ui::theme::Theme::plain(), width);
        for (at, line) in painted.iter().enumerate() {
            let painted_width = lca_tui::engine::text::visible_width(line);
            assert!(
                painted_width <= width,
                "row {at} painted {painted_width} columns into a {width}-column \
                 box: {line:?}"
            );
        }
        assert!(
            painted.iter().any(|line| line.contains("question 39")),
            "the last turn's text survives a {width}-column frame"
        );
    }
}

/// The live leaf is the row the cursor opens on, even at turn 40 - the
/// anchoring rule that makes the fix usable on a long session.
#[test]
fn the_cursor_opens_on_the_last_turn_of_a_long_session() {
    let (_scratch, store, session) = forty_turn_session();
    let picker = lca_ui::state::tree_picker(tree_rows(&store, &session), "default");
    assert_eq!(
        picker.selected_id().as_deref(),
        Some("a39"),
        "the live leaf anchors the cursor"
    );
}

/// The display view a real resume reads must still resolve the whole
/// session: the layout fix never touches the log.
#[test]
fn the_session_log_still_resolves_every_record() {
    let (_scratch, store, session) = forty_turn_session();
    let shown = store.resolved(&session, ViewMode::Display).unwrap();
    // 80 turns plus the `session-start` framing record the store writes.
    assert_eq!(shown.records.len(), 81);
    assert_eq!(
        shown
            .records
            .iter()
            .filter(|record| matches!(record, Record::Assistant { .. }))
            .count(),
        40,
        "every assistant record still resolves"
    );
}
