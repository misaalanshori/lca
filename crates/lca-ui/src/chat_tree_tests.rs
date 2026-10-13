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
        label: None,
        live: false,
    };
    let picker = TreePicker::new(
        vec![
            row("u1", 0, TreeRowKind::User, "first"),
            row("a1", 1, TreeRowKind::Assistant, "reply"),
            row("u2", 1, TreeRowKind::User, "second path"),
        ],
        TreeFilter::Default,
    );
    let painted = strip(&picker.paint_rows(&crate::theme::Theme::plain())).join("\n");
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
        label: label.map(str::to_string),
        live: false,
    };
    let rows = vec![
        row("u", TreeRowKind::User, "ask", None),
        row("a", TreeRowKind::Assistant, "answer", Some("mark")),
        row("t", TreeRowKind::Tool, "read", None),
        row("c", TreeRowKind::Assistant, "(tool call)", None),
    ];
    let texts = |filter: TreeFilter| {
        strip(&TreePicker::new(rows.clone(), filter).paint_rows(&crate::theme::Theme::plain()))
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
        label: None,
        live: false,
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
                label: None,
                live: true,
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
        label: None,
        live: false,
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
    let painted = picker.paint_rows(&theme).join("\n");
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
        label: None,
        live,
    };
    let picker = TreePicker::new(
        vec![row("a", 0, true), row("b", 1, true), row("c", 1, false)],
        TreeFilter::All,
    );
    let painted = picker.paint_rows(&theme);
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
        label: None,
        live,
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
        label: None,
        live,
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
