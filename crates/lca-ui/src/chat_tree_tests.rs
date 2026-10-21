//! Tree navigator journeys (gh #231, ceiling split from
//! `chat_command_tests.rs`): paint, filter, fold, labels, branches.
//! Shares the `chat_tests` helpers.

use super::{chat, options, strip, tree_row};
use lca_tui::engine::keybindings::KeybindingsManager;

use super::super::Chat;
use std::sync::Arc;

// Verifies: gh #231 - the DAG navigator paints branch connectors: a
// fork shows `├─`/`└─` with a `│` gutter, role markers name each row.
#[test]
fn tree_paints_connectors_and_role_markers() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, kind: TreeRowKind, text: &str| TreeRow {
        id: id.to_string(),
        depth,
        kind,
        text: text.to_string(),
        call: None,
        label: None,
        live: false,
        live_leaf: false,
    };
    let picker = TreePicker::new(
        vec![
            row("u1", 0, TreeRowKind::User, "first"),
            row("a1", 1, TreeRowKind::Assistant, "reply"),
            row("u2", 1, TreeRowKind::User, "second path"),
        ],
        TreeFilter::Default,
    );
    let painted = strip(&picker.paint_rows(&crate::theme::Theme::plain(), usize::MAX)).join("\n");
    assert!(painted.contains("user: first"), "role markers:\n{painted}");
    assert!(
        painted.contains("assistant: reply"),
        "role markers:\n{painted}"
    );
    assert!(
        painted.contains("├─") && painted.contains("└─"),
        "fork connectors:\n{painted}"
    );
}

// Verifies: gh #231 - filter modes hide by kind (default drops tool
// rows, no-tools drops tool-call assistants too, user-only keeps
// prompts, labeled-only keeps marks, all shows everything).
#[test]
fn tree_filters_by_kind_and_label() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, kind: TreeRowKind, text: &str, label: Option<&str>| TreeRow {
        id: id.to_string(),
        depth: 0,
        kind,
        text: text.to_string(),
        call: None,
        label: label.map(str::to_string),
        live: false,
        live_leaf: false,
    };
    let rows = vec![
        row("u", TreeRowKind::User, "ask", None),
        row("a", TreeRowKind::Assistant, "answer", Some("mark")),
        row("t", TreeRowKind::Tool, "read", None),
        row("c", TreeRowKind::Assistant, "(tool call)", None),
    ];
    let texts = |filter: TreeFilter| {
        strip(
            &TreePicker::new(rows.clone(), filter)
                .paint_rows(&crate::theme::Theme::plain(), usize::MAX),
        )
        .join("\n")
    };
    let default = texts(TreeFilter::Default);
    assert!(
        default.contains("ask") && default.contains("answer") && !default.contains("tool: read"),
        "default hides tool rows:\n{default}"
    );
    let no_tools = texts(TreeFilter::NoTools);
    assert!(
        !no_tools.contains("tool: read") && !no_tools.contains("(tool call)"),
        "no-tools hides tool traffic:\n{no_tools}"
    );
    let user_only = texts(TreeFilter::UserOnly);
    assert!(
        user_only.contains("ask") && !user_only.contains("answer"),
        "user-only keeps prompts:\n{user_only}"
    );
    let labeled = texts(TreeFilter::LabeledOnly);
    assert!(
        labeled.contains("answer") && !labeled.contains("ask"),
        "labeled-only keeps marks:\n{labeled}"
    );
    let all = texts(TreeFilter::All);
    assert!(
        all.contains("tool: read") && all.contains("(tool call)"),
        "all shows everything:\n{all}"
    );
}

// Verifies: gh #231 - folding hides a subtree and unfolding restores
// it; the selection survives a filter cycle by record id.
#[test]
fn tree_fold_hides_and_filter_cycle_keeps_selection() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize| TreeRow {
        id: id.to_string(),
        depth,
        kind: TreeRowKind::User,
        text: id.to_string(),
        call: None,
        label: None,
        live: false,
        live_leaf: false,
    };
    let mut picker = TreePicker::new(
        vec![row("a", 0), row("b", 1), row("c", 1), row("d", 0)],
        TreeFilter::Default,
    );
    assert_eq!(picker.visible_len(), 4);
    picker.toggle_fold_at(0); // fold "a": its children hide
    assert_eq!(picker.visible_len(), 2, "b and c hide");
    picker.toggle_fold_at(0); // unfold: the subtree returns
    assert_eq!(picker.visible_len(), 4, "b and c return");
    picker.selected = 2; // "c"
    picker.set_filter(TreeFilter::UserOnly);
    assert_eq!(
        picker.selected_id().as_deref(),
        Some("c"),
        "selection rides the id"
    );
    picker.set_filter(TreeFilter::LabeledOnly); // nothing labeled
    assert_eq!(picker.visible_len(), 0, "empty views hold");
}

// Verifies: gh #231 - Left folds a subtree (Right unfolds), `f`
// cycles the filter forward with the selection riding its record.
#[test]
fn tree_fold_and_filter_cycle_through_keys() {
    let mut opts = options();
    opts.hooks.session_tree = Some(Arc::new(|| {
        vec![
            tree_row("a", 0, "first"),
            tree_row("b", 1, "child one"),
            tree_row("c", 1, "child two"),
        ]
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/tree".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    let picker = chat.tree_picker.as_ref().expect("open");
    assert_eq!(picker.visible_len(), 3);
    chat.handle_key("\x1b[D"); // Left: fold "a"
    assert_eq!(
        chat.tree_picker.as_ref().expect("open").visible_len(),
        1,
        "the children hide"
    );
    chat.handle_key("\x1b[C"); // Right on a folded node opens it
    assert_eq!(
        chat.tree_picker.as_ref().expect("open").visible_len(),
        3,
        "the children return"
    );
    chat.handle_key("j");
    chat.handle_key("j"); // on "c"
    chat.handle_key("f"); // default -> no-tools
    let picker = chat.tree_picker.as_ref().expect("open");
    assert_eq!(
        picker.filter,
        crate::chat_pickers::TreeFilter::NoTools,
        "the filter cycled"
    );
    assert_eq!(
        picker.selected_id().as_deref(),
        Some("c"),
        "the selection rides its record"
    );
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(
        viewport.contains("no-tools"),
        "the header names it:\n{viewport}"
    );
}

// Verifies: gh #231 - `e` edits the selected row's bookmark through
// the host hook: typing lands in the buffer, Enter commits and the
// rebuilt tree carries the mark.
#[test]
fn tree_label_edit_commits_through_the_hook() {
    use std::sync::Mutex;
    let saved = Arc::new(Mutex::new(Vec::new()));
    let written = saved.clone();
    let mut opts = options();
    opts.hooks.session_tree = Some(Arc::new(|| vec![tree_row("r1", 0, "first")]));
    opts.hooks.label_record = Some(Arc::new(move |id: &str, name: &str| {
        written
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((id.to_string(), name.to_string()));
        Ok(format!("bookmarked '{name}'"))
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/tree".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.handle_key("e");
    assert!(
        chat.tree_picker.as_ref().expect("open").editing.is_some(),
        "the buffer opens"
    );
    for c in "mine".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        saved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        &[("r1".to_string(), "mine".to_string())],
        "the hook bookmarks by record id"
    );
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("bookmarked 'mine'"),
        "the hook's notice shows"
    );
    assert!(
        chat.tree_picker.as_ref().expect("open").editing.is_none(),
        "the buffer closed"
    );
}

// Verifies: FR-UI-16 - `/tree` browses the live session's entry
// tree and selecting a row branches there in place (gh #37 phase 3:
// record rows replace the fork-directory rows; fork switching lives
// on `/resume`, forking on `/fork`).
#[test]
fn tree_browses_entry_branches_and_selection_branches_there() {
    let mut options = options();
    options.hooks.session_tree = Some(Arc::new(|| {
        vec![
            tree_row("r1", 0, "first question"),
            crate::state::TreeRow {
                id: "r2".to_string(),
                depth: 1,
                kind: crate::state::TreeRowKind::User,
                text: "second question".to_string(),
                call: None,
                label: None,
                live: true,
                live_leaf: false,
            },
        ]
    }));
    options.hooks.branch_here = Some(Arc::new(|id: &str| {
        assert_eq!(id, "r2");
        Some(vec![])
    }));
    options.hooks.fork_at = Some(Arc::new(|n: usize| crate::state::ForkReport {
        id: Some("newbranch".to_string()),
        notice: format!("forked at {n}: newbranch"),
    }));
    let mut chat = Chat::new(options, Arc::new(KeybindingsManager::new()));
    for c in "/tree".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.tree_picker.is_some());
    let viewport = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(viewport.contains("first question"), "{viewport}");
    assert!(viewport.contains("second question"), "{viewport}");
    chat.handle_key("j");
    chat.handle_key("\r");
    assert!(
        chat.last_notice().as_deref().unwrap().contains("branched"),
        "{:?}",
        chat.last_notice()
    );
    for c in "/fork 1".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(
        chat.last_notice()
            .as_deref()
            .unwrap()
            .contains("forked at 1"),
        "{:?}",
        chat.last_notice()
    );
}

// Verifies: gh #248 - tree rows carry pi's semantic roles: dim
// gutters and connectors, accent user markers, success assistant
// markers, muted tools, warning bookmarks, border-accent compactions.
#[test]
fn tree_rows_carry_role_colors() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    use crate::theme::{Role, Theme};
    let theme = Theme::colored();
    let row = |id: &str, depth: usize, kind: TreeRowKind, text: &str| TreeRow {
        id: id.to_string(),
        depth,
        kind,
        text: text.to_string(),
        call: None,
        label: None,
        live: false,
        live_leaf: false,
    };
    let picker = TreePicker::new(
        vec![
            row("u1", 0, TreeRowKind::User, "first"),
            row("a1", 1, TreeRowKind::Assistant, "reply"),
            row("t1", 1, TreeRowKind::Tool, "read"),
            row("c1", 0, TreeRowKind::Compaction, "summary"),
        ],
        TreeFilter::All,
    );
    let painted = picker.paint_rows(&theme, usize::MAX).join("\n");
    assert!(
        painted.contains(&theme.role(Role::Accent)("user: ")),
        "user markers read accent:\n{painted}"
    );
    assert!(
        painted.contains(&theme.role(Role::Success)("assistant: ")),
        "assistant markers read success:\n{painted}"
    );
    assert!(
        painted.contains(&theme.role(Role::Dim)("├─ ")),
        "connectors read dim:\n{painted}"
    );
    assert!(
        painted.contains(&theme.role(Role::BorderAccent)("compaction: summary")),
        "compactions read border-accent:\n{painted}"
    );
}

// Verifies: gh #248 - rows on the live chain carry pi's active-path
// marker (`• ` in accent); history off the chain carries none.
#[test]
fn tree_marks_the_active_path() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    use crate::theme::{Role, Theme};
    use lca_tui::engine::text::strip_terminal_sequences;
    let theme = Theme::colored();
    let row = |id: &str, depth: usize, live: bool| TreeRow {
        id: id.to_string(),
        depth,
        kind: TreeRowKind::User,
        text: id.to_string(),
        call: None,
        label: None,
        live,
        // The rows these closures build mark the chain, not the tip;
        // the leaf-exemption row sets it explicitly.
        live_leaf: false,
    };
    let picker = TreePicker::new(
        vec![row("a", 0, true), row("b", 1, true), row("c", 1, false)],
        TreeFilter::All,
    );
    let painted = picker.paint_rows(&theme, usize::MAX);
    let mark = theme.role(Role::Accent)("• ");
    let live_lines: Vec<String> = painted
        .iter()
        .filter(|line| strip_terminal_sequences(line).contains("• "))
        .cloned()
        .collect();
    assert_eq!(live_lines.len(), 2, "the live chain marks:\n{painted:?}");
    for line in &live_lines {
        assert!(line.contains(&mark), "the mark reads accent:\n{line}");
    }
}

// Verifies: gh #248 - opening `/tree` anchors the cursor on the live
// leaf (the deepest live row), not row 0; no live row keeps row 0.
#[test]
fn tree_opens_on_the_live_leaf() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, live: bool| TreeRow {
        id: id.to_string(),
        depth,
        kind: TreeRowKind::User,
        text: id.to_string(),
        call: None,
        label: None,
        live,
        // The rows these closures build mark the chain, not the tip;
        // the leaf-exemption row sets it explicitly.
        live_leaf: false,
    };
    let picker = TreePicker::new(
        vec![row("a", 0, true), row("b", 1, true), row("c", 1, false)],
        TreeFilter::All,
    );
    assert_eq!(
        picker.selected_id().as_deref(),
        Some("b"),
        "the cursor rides the live leaf"
    );
    let bare = TreePicker::new(
        vec![row("a", 0, false), row("b", 0, false)],
        TreeFilter::All,
    );
    assert_eq!(bare.selected, 0, "no live row keeps row 0");
}

// Verifies: gh #248 - the open `/tree` modal paints styled rows
// into the viewport (raw SGR reaches the frame) with the cursor on
// the live leaf, and no row touches the box's left edge.
#[test]
fn tree_modal_paints_styled_rows_with_cursor_on_live() {
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, live: bool| TreeRow {
        id: id.to_string(),
        depth,
        kind: TreeRowKind::User,
        text: format!("prompt {id}"),
        call: None,
        label: None,
        live,
        // The rows these closures build mark the chain, not the tip;
        // the leaf-exemption row sets it explicitly.
        live_leaf: false,
    };
    let mut chat = chat();
    chat.screen_mode = true;
    chat.tree_picker = Some(crate::chat_pickers::TreePicker::new(
        vec![row("a", 0, true), row("b", 1, true)],
        crate::chat_pickers::TreeFilter::All,
    ));
    let frame = chat.viewport(100, 30, 0);
    let raw = frame.join("\n");
    let text = strip(&frame).join("\n");
    assert!(text.contains("prompt b"), "the rows paint:\n{text}");
    assert!(raw.contains("\x1b["), "styled rows reach the frame");
    let cursor = text
        .lines()
        .find(|line| line.contains("prompt b"))
        .expect("the live leaf row");
    assert!(
        cursor.contains('>'),
        "the cursor rides the live leaf:\n{text}"
    );
    assert!(
        text.contains(" Message branches"),
        "the header insets one cell:\n{text}"
    );
}

// --- gh #394: the layout math ---------------------------------------
//
// The four rules are pi's `tree-selector.ts` rules; the shape of each row
// is pinned here at string level, because the bug was purely geometric:
// one gutter per *ancestor* marched a linear session off the screen.

// Verifies: gh #394 fix 1 - a 40-turn linear session (81 rows, one child
// each) paints every row at column 0. Before the fix `depth` counted
// visible ancestors, so row 80 carried 160 gutter columns and the whole
// session marched diagonally off the viewport.
#[test]
fn a_forty_turn_linear_session_paints_every_row_at_column_zero() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let mut rows = Vec::new();
    for turn in 0..40 {
        let kind = if turn % 2 == 0 {
            TreeRowKind::User
        } else {
            TreeRowKind::Assistant
        };
        let depth = turn;
        rows.push(TreeRow {
            id: format!("r{turn}"),
            depth,
            kind,
            text: format!("turn {turn}"),
            call: None,
            label: None,
            live: false,
            live_leaf: false,
        });
    }
    let painted = strip(
        &TreePicker::new(rows, TreeFilter::Default)
            .paint_rows(&crate::theme::Theme::plain(), usize::MAX),
    );
    assert_eq!(painted.len(), 40, "every turn is a row");
    for (turn, line) in painted.iter().enumerate() {
        let expected = if turn % 2 == 0 {
            format!("user: turn {turn}")
        } else {
            format!("assistant: turn {turn}")
        };
        assert_eq!(
            line, &expected,
            "row {turn} must sit at column 0, with no gutter or connector"
        );
    }
}

// Verifies: gh #394 fix 1 - a bush indents exactly at its forks: a
// branch point's children carry `├─`/`└─`, the connector column keeps a
// `│` gutter for a later sibling's descendants, and a single-child chain
// under a fresh fork takes one extra grouping level.
#[test]
fn a_bush_paints_connectors_and_one_grouping_level_under_each_fork() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, kind: TreeRowKind, text: &str| TreeRow {
        id: id.to_string(),
        depth,
        kind,
        text: text.to_string(),
        call: None,
        label: None,
        live: false,
        live_leaf: false,
    };
    let rows = vec![
        row("u1", 0, TreeRowKind::User, "root"),
        row("a1", 1, TreeRowKind::Assistant, "branch one"),
        row("g1", 2, TreeRowKind::Tool, "grandchild"),
        row("a2", 1, TreeRowKind::Assistant, "branch two"),
        row("a3", 1, TreeRowKind::Assistant, "branch three"),
    ];
    let painted = strip(
        &TreePicker::new(rows, TreeFilter::All)
            .paint_rows(&crate::theme::Theme::plain(), usize::MAX),
    );
    // The fork glyph is `⊟` where the branch has a visible child to fold
    // and `─` where it has none (gh #394 fix 4); the grandchild sits one
    // grouping level in, under the connector column's `│` gutter.
    assert_eq!(
        painted,
        vec![
            "user: root".to_string(),
            "├⊟ assistant: branch one".to_string(),
            "│     tool: grandchild".to_string(),
            "├─ assistant: branch two".to_string(),
            "└─ assistant: branch three".to_string(),
        ],
        "the bush's exact frame"
    );
}

// Verifies: gh #394 fix 2 - the branch holding the live leaf sorts first at
// every level, and equal-priority siblings keep their stored order, so two
// renders of one session are byte-identical (no tie-order churn).
#[test]
fn the_active_trunk_sorts_first_at_every_level_and_stays_deterministic() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, live: bool| TreeRow {
        id: id.to_string(),
        depth,
        kind: TreeRowKind::User,
        text: id.to_string(),
        call: None,
        label: None,
        live,
        // The rows these closures build mark the chain, not the tip;
        // the leaf-exemption row sets it explicitly.
        live_leaf: false,
    };
    // Two roots, two children each; the live path runs through the second
    // root's second child.
    let rows = vec![
        row("cold-root", 0, false),
        row("cold-kid", 1, false),
        row("live-root", 0, true),
        row("idle-kid", 1, false),
        row("live-kid", 1, true),
    ];
    let picker = TreePicker::new(rows, TreeFilter::Default);
    let texts: Vec<String> = strip(&picker.paint_rows(&crate::theme::Theme::plain(), usize::MAX))
        .iter()
        .map(|line| {
            line.trim_start_matches(['│', ' ', '├', '└', '─', '⊟', '•'])
                .trim_start_matches("user: ")
                .trim_end_matches('◀')
                .trim()
                .to_string()
        })
        .collect();
    assert_eq!(
        texts,
        vec!["live-root", "live-kid", "idle-kid", "cold-root", "cold-kid"],
        "the live root leads, and its live child leads its siblings"
    );
    let again = TreePicker::new(picker.rows.clone(), TreeFilter::Default);
    assert_eq!(
        strip(&picker.paint_rows(&crate::theme::Theme::plain(), usize::MAX)),
        strip(&again.paint_rows(&crate::theme::Theme::plain(), usize::MAX)),
        "two renders of one session are identical"
    );
}

// Verifies: gh #394 fix 3 - a filter change that hides the selected row
// lands the cursor on the nearest *visible* ancestor (the user prompt that
// owns that turn), not on whatever row now sits at the old index, and not
// on a clamp to the end.
#[test]
fn a_filter_change_anchors_the_cursor_to_the_nearest_visible_ancestor() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, kind: TreeRowKind| TreeRow {
        id: id.to_string(),
        depth,
        kind,
        text: id.to_string(),
        call: None,
        label: None,
        live: false,
        live_leaf: false,
    };
    let rows = vec![
        row("u1", 0, TreeRowKind::User),
        row("a1", 1, TreeRowKind::Assistant),
        row("t1", 2, TreeRowKind::Tool),
        row("u2", 0, TreeRowKind::User),
        row("a2", 1, TreeRowKind::Assistant),
    ];
    let mut picker = TreePicker::new(rows, TreeFilter::All);
    // Select the tool row: three levels in, under a1, under u1.
    picker.selected = 2;
    assert_eq!(picker.selected_id().as_deref(), Some("t1"));
    // `user-only` hides a1 and t1; the walk reaches u1, their ancestor.
    picker.set_filter(TreeFilter::UserOnly);
    assert_eq!(
        picker.selected_id().as_deref(),
        Some("u1"),
        "the cursor rides the governing prompt, never a stranger row"
    );
    // A filter that keeps nothing leaves no cursor to point anywhere.
    picker.set_filter(TreeFilter::All);
    picker.selected = 1;
    picker.set_filter(TreeFilter::LabeledOnly);
    assert_eq!(picker.visible_len(), 0);
    assert_eq!(
        picker.selected_id().as_deref(),
        None,
        "an empty view cannot strand a cursor on a hidden row"
    );
}

// Verifies: gh #394 fix 3 - a filtered tree recomputes its indentation from
// the *visible* rows: hiding the only sibling flattens the survivor to
// column 0, because nothing branches any more.
#[test]
fn a_filter_change_recomputes_the_indent_from_the_visible_tree() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, kind: TreeRowKind| TreeRow {
        id: id.to_string(),
        depth,
        kind,
        text: id.to_string(),
        call: None,
        label: None,
        live: false,
        live_leaf: false,
    };
    let rows = vec![
        row("u1", 0, TreeRowKind::User),
        row("a1", 1, TreeRowKind::Assistant),
        row("u2", 1, TreeRowKind::User),
    ];
    let fork = strip(
        &TreePicker::new(rows.clone(), TreeFilter::All)
            .paint_rows(&crate::theme::Theme::plain(), usize::MAX),
    );
    assert!(
        fork[1].starts_with("├─") && fork[2].starts_with("└─"),
        "the fork draws:\n{fork:#?}"
    );
    // `user-only` hides a1, leaving u1 with a single visible child: the
    // survivor flattens to column 0 (pi's recompute-after-filter).
    let flat = strip(
        &TreePicker::new(rows, TreeFilter::UserOnly)
            .paint_rows(&crate::theme::Theme::plain(), usize::MAX),
    );
    assert_eq!(
        flat,
        vec!["user: u1".to_string(), "user: u2".to_string()],
        "the hidden sibling releases the indent:\n{flat:#?}"
    );
}

// Verifies: gh #394 fix 4 - a folded subtree paints `⊞` in the connector
// cell (an expanded segment start paints `⊟`), costing no extra column, and
// a folded root with no connector carries its glyph in the marker slot.
#[test]
fn fold_glyphs_paint_in_the_connector_cell_and_on_bare_roots() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, kind: TreeRowKind| TreeRow {
        id: id.to_string(),
        depth,
        kind,
        text: id.to_string(),
        call: None,
        label: None,
        live: false,
        live_leaf: false,
    };
    let rows = vec![
        row("u1", 0, TreeRowKind::User),
        row("a1", 1, TreeRowKind::Assistant),
        row("a2", 1, TreeRowKind::Assistant),
        row("g1", 2, TreeRowKind::Tool),
    ];
    let mut picker = TreePicker::new(rows, TreeFilter::All);
    let open = strip(&picker.paint_rows(&crate::theme::Theme::plain(), usize::MAX));
    assert!(
        open[1].starts_with("├─"),
        "a branch with no children of its own shows the plain dash:\n{open:#?}"
    );
    assert!(
        open[2].starts_with("└⊟"),
        "the last branch is a segment start with a chain under it:\n{open:#?}"
    );
    assert!(picker.toggle_fold_at(2), "the last branch folds");
    let folded = strip(&picker.paint_rows(&crate::theme::Theme::plain(), usize::MAX));
    assert_eq!(
        folded,
        vec![
            "user: u1".to_string(),
            "├─ assistant: a1".to_string(),
            "└⊞ assistant: a2".to_string(),
        ],
        "the folded branch shows `⊞` in the dash cell, zero extra columns:\n{folded:#?}"
    );
    // A root with children hides everything under it; with no connector to
    // carry the glyph it takes the marker slot instead.
    let mut root = TreePicker::new(
        vec![
            row("r", 0, TreeRowKind::User),
            row("kid", 1, TreeRowKind::User),
        ],
        TreeFilter::All,
    );
    assert!(root.toggle_fold_at(0));
    assert_eq!(
        strip(&root.paint_rows(&crate::theme::Theme::plain(), usize::MAX)),
        vec!["⊞ user: r".to_string()],
        "a folded root still says it is folded"
    );
}

// Verifies: gh #394 fix 4 - the keys are dual-action: fold while there is
// something to fold, then jump to the branch segment's start; unfold, then
// jump down to the next fork. A leaf's key falls through to the jump.
#[test]
fn the_fold_key_folds_then_jumps_and_the_unfold_key_unfolds_then_jumps() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, kind: TreeRowKind| TreeRow {
        id: id.to_string(),
        depth,
        kind,
        text: id.to_string(),
        call: None,
        label: None,
        live: false,
        live_leaf: false,
    };
    // u1 -> {a1 -> g1, a2}: a fork with a chain under its first branch.
    let rows = vec![
        row("u1", 0, TreeRowKind::User),
        row("a1", 1, TreeRowKind::Assistant),
        row("g1", 2, TreeRowKind::Assistant),
        row("a2", 1, TreeRowKind::Assistant),
    ];
    let mut picker = TreePicker::new(rows, TreeFilter::All);
    picker.selected = 3; // a2, a leaf sibling of a1
    assert!(
        picker.fold_or_up(),
        "on a leaf the key jumps up the segment"
    );
    assert_eq!(
        picker.selected_id().as_deref(),
        Some("u1"),
        "pi's walk ascends past the rooted segment to the tree's root"
    );
    // From the root the same key folds the whole subtree; unfolding
    // restores it with the cursor on the same record.
    assert!(picker.fold_or_up(), "on the root, the key folds");
    assert_eq!(picker.visible_len(), 1, "the subtree hides");
    assert!(picker.unfold_or_down(), "the folded root unfolds");
    assert_eq!(picker.visible_len(), 4, "the subtree returns");
    assert_eq!(picker.selected_id().as_deref(), Some("u1"));
    // On a fork with a chain under it, the key folds that fork.
    picker.selected = 1; // a1 has no children: not foldable
    assert!(
        picker.fold_or_up(),
        "a non-foldable row still answers with a jump"
    );
}

// Verifies: gh #394 fix 4 - the cursor gutter stays pinned while the prefix
// and body pan: a deep selection in a narrow box keeps its gutter cell and
// the tree's text scrolls left. No row is ever wider than the box, and a
// shallow pick never pans.
#[test]
fn the_gutter_is_pinned_and_the_body_pans_in_a_narrow_box() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, text: &str| TreeRow {
        id: id.to_string(),
        depth,
        kind: TreeRowKind::Assistant,
        text: text.to_string(),
        call: None,
        label: None,
        live: false,
        live_leaf: false,
    };
    // Six nested forks: each level's last child forks again, so the deepest
    // row's indent is five levels (15 columns) plus its own connector.
    let mut rows = vec![row("root", 0, "root prompt")];
    let mut depth = 0;
    for level in 0..6 {
        depth += 1;
        rows.push(row(&format!("l{level}"), depth, "shallow text"));
        rows.push(row(&format!("deep{level}"), depth, &"D".repeat(60)));
    }
    let mut picker = TreePicker::new(rows, TreeFilter::All);
    let width = 30;
    picker.selected = picker.visible_len() - 1;
    assert!(
        picker
            .selected_id()
            .as_deref()
            .is_some_and(|id| id.starts_with("deep")),
        "the deepest row is selected"
    );
    let painted = picker.paint_rows(&crate::theme::Theme::plain(), width);
    for line in &painted {
        assert!(
            lca_tui::engine::text::visible_width(line) <= width,
            "a row may never exceed the box: {line:?}"
        );
    }
    // The pinned gutter is the caller's cursor cell; the pan lives in the
    // prefix and body, so the text has visibly scrolled left.
    let selected = &painted[picker.selected];
    assert!(
        !selected.trim_start().starts_with("assistant: "),
        "the selected row's text panned out from under the anchor: {selected:?}"
    );
    picker.selected = 0;
    let shallow = picker.paint_rows(&crate::theme::Theme::plain(), width);
    assert!(
        shallow[0].contains("root prompt"),
        "nothing pans while the anchor has room: {shallow:?}"
    );
}

// Verifies: gh #394 fix 4 - the dual-action keys are bound: alt+left folds
// then jumps up, alt+right unfolds then jumps down, and plain left/right
// keep their gh #231 meaning (fold, else parent; child, else fold).
#[test]
fn the_dual_action_fold_keys_are_bound_alongside_the_plain_arrows() {
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, kind: TreeRowKind| TreeRow {
        id: id.to_string(),
        depth,
        kind,
        text: id.to_string(),
        call: None,
        label: None,
        live: false,
        live_leaf: false,
    };
    let rows = vec![
        row("u1", 0, TreeRowKind::User),
        row("a1", 1, TreeRowKind::Assistant),
        row("a2", 1, TreeRowKind::Assistant),
        row("g1", 2, TreeRowKind::Tool),
    ];
    let mut opts = options();
    opts.hooks.session_tree = Some(Arc::new(move || rows.clone()));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/tree".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    // The Default filter hides tool rows (gh #231), so the bush is u1 with
    // its two branches: a2 is the last one.
    assert_eq!(chat.tree_picker.as_ref().expect("open").visible_len(), 3);
    // On the last branch (a2) alt+left walks to the topmost segment start.
    chat.handle_key("j");
    chat.handle_key("j");
    assert_eq!(
        chat.tree_picker
            .as_ref()
            .expect("open")
            .selected_id()
            .as_deref(),
        Some("a2")
    );
    chat.handle_key("\x1b[1;3D"); // alt+left
    assert_eq!(
        chat.tree_picker
            .as_ref()
            .expect("open")
            .selected_id()
            .as_deref(),
        Some("u1"),
        "alt+left jumped the segment"
    );
    // On the root, alt+left folds it.
    chat.handle_key("\x1b[1;3D");
    assert_eq!(
        chat.tree_picker.as_ref().expect("open").visible_len(),
        1,
        "alt+left folded the root"
    );
    // alt+right unfolds it again.
    chat.handle_key("\x1b[1;3C");
    assert_eq!(
        chat.tree_picker.as_ref().expect("open").visible_len(),
        3,
        "alt+right unfolded it"
    );
    // Plain left still means "fold, else climb to the parent".
    chat.handle_key("j");
    chat.handle_key("j"); // a2
    chat.handle_key("\x1b[D");
    assert_eq!(
        chat.tree_picker
            .as_ref()
            .expect("open")
            .selected_id()
            .as_deref(),
        Some("u1"),
        "a non-foldable leaf's plain left climbs to its parent"
    );
    // The hint names the dual-action keys, so the binding is discoverable.
    let viewport = strip(&chat.viewport(120, 30, 0)).join("\n");
    assert!(
        viewport.contains("alt+") || viewport.contains("⌥"),
        "the hint names the fold keys:\n{viewport}"
    );
}

// Verifies: gh #394 fix 3 - pi's textless-dispatch rule: an assistant row
// that is nothing but tool calls hides under `default` and `no-tools`,
// because its tool rows already say what happened. The live *leaf* is the
// one exemption (the cursor's home); the rest of the live chain hides like
// any other dispatch row, which is what pi's `currentLeafId` check means.
#[test]
fn a_textless_tool_dispatch_row_hides_unless_it_is_the_live_leaf() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let row = |id: &str, depth: usize, kind: TreeRowKind, text: &str, leaf: bool| TreeRow {
        id: id.to_string(),
        depth,
        kind,
        text: text.to_string(),
        call: None,
        label: None,
        live: leaf,
        live_leaf: leaf,
    };
    let rows = vec![
        row("u1", 0, TreeRowKind::User, "do it", false),
        row("a1", 1, TreeRowKind::Assistant, "(tool call)", false),
        row("t1", 2, TreeRowKind::Tool, "read", false),
        row("a2", 1, TreeRowKind::Assistant, "here is the answer", true),
    ];
    for filter in [TreeFilter::Default, TreeFilter::NoTools] {
        let texts: Vec<String> = strip(
            &TreePicker::new(rows.clone(), filter)
                .paint_rows(&crate::theme::Theme::plain(), usize::MAX),
        );
        assert!(
            !texts.iter().any(|line| line.contains("(tool call)")),
            "{:?} hides the empty dispatch row:\n{texts:#?}",
            filter.name()
        );
        assert!(
            texts.iter().any(|line| line.contains("here is the answer")),
            "{:?} keeps real assistant text:\n{texts:#?}",
            filter.name()
        );
    }
    // The same row as the live leaf stays, so the cursor has a home.
    let leaf_only = vec![
        row("u1", 0, TreeRowKind::User, "do it", false),
        row("a1", 1, TreeRowKind::Assistant, "(tool call)", true),
    ];
    let painted = strip(
        &TreePicker::new(leaf_only, TreeFilter::Default)
            .paint_rows(&crate::theme::Theme::plain(), usize::MAX),
    );
    assert!(
        painted.iter().any(|line| line.contains("(tool call)")),
        "the live leaf is never hidden:\n{painted:#?}"
    );
    // A mid-chain dispatch row is live but not the leaf: it hides, so a
    // 40-turn session with a tool call per turn does not double every turn.
    let mid_chain = vec![
        row("u1", 0, TreeRowKind::User, "go", false),
        {
            let mut row = row("a1", 1, TreeRowKind::Assistant, "(tool call)", false);
            row.live = true;
            row
        },
        row("a2", 1, TreeRowKind::Assistant, "final answer", true),
    ];
    let painted = strip(
        &TreePicker::new(mid_chain, TreeFilter::Default)
            .paint_rows(&crate::theme::Theme::plain(), usize::MAX),
    );
    assert!(
        !painted.iter().any(|line| line.contains("(tool call)")),
        "a live-but-not-leaf dispatch row hides:\n{painted:#?}"
    );
}

// Verifies: gh #394 fix 3 - a tool result row renders its call's resolved
// arguments (`[read: src/main.rs:1-50]`) rather than its opaque content
// head, so the navigator says what was run instead of what came back.
#[test]
fn a_tool_result_row_paints_the_resolved_call_label() {
    use crate::chat_pickers::{TreeFilter, TreePicker};
    use crate::state::{TreeRow, TreeRowKind};
    let rows = vec![
        tree_row("u1", 0, "read the file"),
        TreeRow {
            id: "t1".to_string(),
            depth: 1,
            kind: TreeRowKind::Tool,
            text: "128 lines of output".to_string(),
            call: Some("[read: src/main.rs:1-50]".to_string()),
            label: None,
            live: false,
            live_leaf: false,
        },
    ];
    let painted = strip(
        &TreePicker::new(rows, TreeFilter::All)
            .paint_rows(&crate::theme::Theme::plain(), usize::MAX),
    );
    assert!(
        painted[1].contains("[read: src/main.rs:1-50]"),
        "the call's arguments name the row:\n{painted:#?}"
    );
    assert!(
        !painted[1].contains("128 lines of output"),
        "the opaque head gives way to the resolved call:\n{painted:#?}"
    );
}
