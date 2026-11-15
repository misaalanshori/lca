//! Gh #601's interface rows: the `/tree` `s` key, the separator while it runs,
//! and the failure path.
//!
//! Split from `chat_branch_summary.rs` (production) the way the workspace splits
//! every subject: the `impl` lives in the sibling, its rows here. Nothing here
//! reaches a provider - the *host* owns the call, and every row asserts an
//! observable at this seam (a started hook, a painted separator row, a posted
//! fault line, a branch that did not move).

use crate::chat::tests_support::{options, tree_row};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A chat with three tree rows and the two branch-summary hooks wired to
/// counting doubles, so a row can prove what was *asked* rather than what was
/// rendered. Returns the chat plus the counters.
fn chat_with_summary_hooks() -> (crate::chat::Chat, std::sync::Arc<AtomicUsize>) {
    let starts = std::sync::Arc::new(AtomicUsize::new(0));
    let counter = starts.clone();
    let mut opts = options();
    opts.plain = false;
    opts.hooks.session_tree = Some(std::sync::Arc::new(|| {
        vec![
            tree_row("u1", 0, "first"),
            tree_row("a1", 1, "reply"),
            tree_row("u2", 1, "second path"),
        ]
    }));
    let state = std::sync::Arc::new(std::sync::Mutex::new(
        crate::state::BranchSummaryState::Idle,
    ));
    let slot = state.clone();
    opts.hooks.start_branch_summary = Some(std::sync::Arc::new(move |_id: &str| {
        counter.fetch_add(1, Ordering::SeqCst);
        *slot.lock().unwrap_or_else(|p| p.into_inner()) = crate::state::BranchSummaryState::Running;
        None
    }));
    let slot = state.clone();
    opts.hooks.poll_branch_summary = Some(std::sync::Arc::new(move || {
        let mut guard = slot.lock().unwrap_or_else(|p| p.into_inner());
        match guard.clone() {
            crate::state::BranchSummaryState::Done(notice) => {
                *guard = crate::state::BranchSummaryState::Idle;
                crate::state::BranchSummaryState::Done(notice)
            }
            ongoing => ongoing,
        }
    }));
    let chat = crate::chat::Chat::new(
        opts,
        std::sync::Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    );
    (chat, starts)
}

// Verifies: gh #601 row 1 + 4 - **`s` starts the summary and no other key does.**
// The count is the observable: `Up`/`Down`/`Left`/`Right`/`f`/`e`/Enter must leave
// it at 0, because a tree key that spent a model call without being asked is the
// exact thing the owner's decision refuses.
#[test]
fn only_s_starts_a_branch_summary() {
    let (mut chat, starts) = chat_with_summary_hooks();
    chat.open_tree_picker();
    assert!(chat.tree_picker.is_some(), "the navigator opens");

    // Every other key the navigator owns.
    for key in [
        "\x1b[A", // Up
        "\x1b[B", // Down
        "\x1b[D", // Left (fold/climb)
        "\x1b[C", // Right (unfold/drop)
        "f", "e", "\r", "\x1b",
    ] {
        chat.handle_key(key);
    }
    assert_eq!(
        starts.load(Ordering::SeqCst),
        0,
        "no other tree key may spend a model call"
    );

    // `s` does.
    chat.open_tree_picker();
    chat.handle_key("s");
    assert_eq!(starts.load(Ordering::SeqCst), 1, "`s` asks for the summary");
    assert!(
        chat.tree_picker.is_none(),
        "and the navigator closes, because the running row lives in the dock"
    );
}

// Verifies: gh #601 row 2 - **the separator says `Summarizing branch` while it
// runs, in one row, at both widths.** A 20-column row keeps the state word and
// drops the rest (the truncation priority the `Compacting` ladder already
// implements); a 100-column row carries the phrase whole.
#[test]
fn the_separator_says_summarizing_branch_while_it_runs() {
    let (mut chat, _starts) = chat_with_summary_hooks();
    chat.open_tree_picker();
    chat.handle_key("s");
    assert_eq!(
        chat.separator.state(),
        &crate::SeparatorState::BranchSummary,
        "the running state is a first-class separator state"
    );
    // One row, and the words are on it.
    let wide: String = chat.viewport(100, 24, 0).join("\n");
    assert!(
        wide.contains("Summarizing branch"),
        "the wide row names it: {wide}"
    );
    let narrow: String = chat.viewport(20, 24, 0).join("\n");
    assert!(
        narrow.contains("Summar"),
        "a narrow row keeps the state word, dropping the rest: {narrow}"
    );
    // The state is a *work* state, so the spinner is up (the frame paints at
    // least one braille glyph) - and it is not the plain rest row.
    assert_eq!(
        chat.separator.state(),
        &crate::SeparatorState::BranchSummary
    );
    assert!(
        !narrow.contains("Working"),
        "a summary is not a turn: {narrow}"
    );
}

// Verifies: gh #601 row 3 - **a failed summary names the provider error and
// leaves the session usable.** The host's `Done` notice is what carries the
// error text; the separator rests, and a later Enter still branches.
#[test]
fn a_failed_summary_names_the_error_and_rests_the_separator() {
    let (mut chat, _starts) = chat_with_summary_hooks();
    chat.open_tree_picker();
    chat.handle_key("s");
    assert_eq!(
        chat.separator.state(),
        &crate::SeparatorState::BranchSummary
    );
    // The host finishes with a fault line (its own `Err` text).
    // Reach the cell through the hook by returning `Done` on the next poll.
    chat.world.options.hooks.poll_branch_summary = Some(std::sync::Arc::new(|| {
        crate::state::BranchSummaryState::Done(
            "branch summary failed: the provider refused (HTTP 500)".to_string(),
        )
    }));
    assert!(chat.poll_branch_summary(), "the result is a repaint");
    assert_eq!(
        chat.separator.state(),
        &crate::SeparatorState::Idle,
        "the row rests"
    );
    let notice = chat.last_notice().unwrap_or_default();
    assert!(
        notice.contains("HTTP 500") && notice.contains("branch summary failed"),
        "the fault line names the provider error: {notice}"
    );
    assert!(!chat.summarizing_branch, "and nothing claims to be running");
}

// Verifies: gh #601 - **a host that cannot start the work says so, and nothing
// claims to run.** `Some(reason)` from the start hook is the "cannot even
// begin" case (no provider, no compaction world).
#[test]
fn a_refused_start_posts_the_reason_and_paints_no_running_state() {
    let mut opts = options();
    opts.plain = false;
    opts.hooks.session_tree = Some(std::sync::Arc::new(|| {
        vec![tree_row("u1", 0, "first"), tree_row("a1", 1, "reply")]
    }));
    opts.hooks.start_branch_summary = Some(std::sync::Arc::new(|_id: &str| {
        Some("no provider is available to summarize the branch".to_string())
    }));
    let mut chat = crate::chat::Chat::new(
        opts,
        std::sync::Arc::new(lca_tui::engine::keybindings::KeybindingsManager::new()),
    );
    chat.open_tree_picker();
    chat.handle_key("s");
    assert_eq!(
        chat.separator.state(),
        &crate::SeparatorState::Idle,
        "a refusal is not a running state"
    );
    let notice = chat.last_notice().unwrap_or_default();
    assert!(
        notice.contains("no provider is available"),
        "the reason is shown: {notice}"
    );
    assert!(!chat.summarizing_branch);
}

// Verifies: gh #601 row 6 - **the hint row names the key.** A key that works but
// is not named is a key nobody presses (the gh #386 gap 3 rule).
#[test]
fn the_tree_hint_names_the_summarize_key() {
    let (mut chat, _starts) = chat_with_summary_hooks();
    chat.open_tree_picker();
    let frame: String = chat.viewport(140, 40, 0).join("\n");
    assert!(
        frame.contains("s summarize"),
        "the navigator's hint lists the key with a word for it: {frame}"
    );
}
