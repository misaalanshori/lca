//! NFR-24 guard for gh #398 gap 1: the sign-in URL must be copyable and openable
//! over SSH and in tmux, where a 3-4 row wrapped URL cannot be mouse-selected.
//!
//! # The defect, stated the way the issue states it
//!
//! An OAuth authorization URL is 180-300 characters, so at 80 columns it wraps
//! across three or four rows inside the login modal. Dragging the mouse across
//! those rows in tmux grabs the hard `\n`s at the soft-wrap seams plus the
//! padding and `│` borders the modal draws, and pasting that into a browser
//! gives a broken URL. `OSC 8` click-through - the local-desktop escape hatch -
//! is unavailable exactly there. The URL is the only way to finish the login, so
//! the user had to reconstruct it by hand.
//!
//! Two halves: a one-key copy of the exact unwrapped URL with in-place
//! confirmation, and a short clickable label instead of asking for a mouse
//! selection.
//!
//! # What this file guards, and what lives beside the code
//!
//! The behaviour rows are in `crates/lca-ui/src/chat_auth_url_tests.rs`, because
//! they need the private `chat()` fixture and the private `Chat` fields. This
//! file guards the properties that are about the *shape of the code*, which a
//! behaviour row cannot see:
//!
//! 1. **The copy target is captured from the step, not from the rendered rows.**
//!    That is the entire fix: if the URL were recovered by re-reading the
//!    painted frame, the wrap newlines and pad fragments would come back. A row
//!    with the right answer today would not notice a future edit that
//!    re-introduced row-scraping.
//! 2. **The clickable label is gated on the terminal capability**, and the gate
//!    is the same detection the rest of the interface uses - not a second
//!    answer that could drift.
//! 3. **The hint resolves its key from the registry**, which is #410's lesson
//!    applied before it repeats.
//! 4. **The confirmation is transient and shares the toast window**, so the two
//!    copy notices fade on one clock.
//!
//! Verifies: gh #398 gap 1, FR-UI-5, NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

/// The workspace root, from this crate's manifest dir.
fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// One source file's text.
fn source(relative: &str) -> String {
    let path = crate_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{} is readable: {error}", path.display());
    })
}

/// Strip `//`-prefixed comment lines, so a rule named in prose cannot satisfy a
/// guard that is supposed to check code.
fn code_lines(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: gh #398 gap 1 - **the copy target comes from the step, not from the
// rendered rows**. `apply_login_next` captures the URL when the step arrives;
// `message_copy_text` reads that field.
//
// This is the guard that would catch the regression the whole issue is about:
// anything that re-derives the URL from painted output re-introduces the wrap
// newlines and pad fragments, and the byte-equality row might still pass on a
// short test URL.
#[test]
fn the_copy_target_is_captured_from_the_step_not_from_the_rendered_rows() {
    // The step application lives in `login_step.rs` after the gate-11 split; the
    // rule is the same whichever file holds it, so the guard follows the code
    // rather than pinning a filename.
    let step = code_lines(&source("crates/lca-ui/src/login_step.rs"));
    assert!(
        step.contains("state.auth_url ="),
        "the login step records its URL when it arrives"
    );

    let keys = code_lines(&source("crates/lca-ui/src/chat_keys.rs"));
    // The copy target's first branch is the captured field.
    let copy_target = keys
        .split("pub fn message_copy_text")
        .nth(1)
        .and_then(|rest| rest.split("pub(crate) fn http_url_of").next())
        .expect("message_copy_text is present");
    assert!(
        copy_target.contains("self.world.auth_url"),
        "the copy target is the captured URL, not text scraped back out of a \
         frame:\n{copy_target}"
    );
    // Only the URL branch matters: the *fallback* below it legitimately strips
    // the last assistant message's escape sequences. What must never happen is
    // recovering the URL by re-reading painted output.
    let url_branch = copy_target
        .split("self.world.auth_url")
        .nth(1)
        .and_then(|rest| rest.split("return Some").next())
        .expect("the URL branch is present");
    for forbidden in ["viewport(", "render(", "strip_terminal_sequences"] {
        assert!(
            !url_branch.contains(forbidden),
            "the URL branch never reads rendered output (`{forbidden}`):\n{url_branch}"
        );
    }
}

// Verifies: gh #398 design point 4 - **the clickable label is gated on the
// terminal capability**, and by the interface's own detection.
//
// The rule from the brief: never render a "click to open" label on a terminal
// that cannot click. A hardcoded `true` would look right in a screenshot and be
// a lie on every SSH session, which is the exact audience for this fix.
#[test]
fn the_click_label_is_gated_on_the_shared_hyperlink_detection() {
    let chat = code_lines(&source("crates/lca-ui/src/chat.rs"));
    assert!(
        chat.contains("hyperlinks: lca_tui::engine::terminal::supports_hyperlinks()"),
        "the gate defaults to the interface's one hyperlink detection, so a \
         second answer cannot drift from the renderer's"
    );

    let overlays = code_lines(&source("crates/lca-ui/src/chat_overlays.rs"));
    let hint = overlays
        .split("fn auth_url_hint")
        .nth(1)
        .and_then(|rest| rest.split("pub fn note_url_copied").next())
        .expect("auth_url_hint is present");
    assert!(
        hint.contains("if self.hyperlinks"),
        "the clickable label is behind the capability gate, not unconditional:\n{hint}"
    );
    // Both halves of the gate: the label exists inside it, and the copy hint
    // (which works everywhere) is outside it.
    let gated = hint
        .split("if self.hyperlinks")
        .nth(1)
        .expect("the gate has a body");
    let inside = &gated[..gated.find("parts.push").map_or(gated.len(), |at| at + 40)];
    assert!(
        inside.contains("click to open"),
        "the click label is inside the gate:\n{gated}"
    );
    assert!(
        !inside.contains("to copy"),
        "and the copy hint is not - it works on every terminal:\n{gated}"
    );
}

// Verifies: gh #398 design point 2 / the #410 lesson - **the hint names the live
// binding**. A hardcoded `ctrl+x` would be right until someone rebinds, and then
// it would be a stale hint pointing at a key that no longer copies.
#[test]
fn the_copy_hint_resolves_its_key_from_the_registry() {
    let overlays = code_lines(&source("crates/lca-ui/src/chat_overlays.rs"));
    let hint = overlays
        .split("fn auth_url_hint")
        .nth(1)
        .and_then(|rest| rest.split("pub fn note_url_copied").next())
        .expect("auth_url_hint is present");
    assert!(
        hint.contains("display_keys(\"app.message.copy\")"),
        "the hint asks the live manager:\n{hint}"
    );
    for hardcoded in ["ctrl+x", "Ctrl+X", "ctrl+X"] {
        assert!(
            !hint.contains(hardcoded),
            "no hardcoded `{hardcoded}` - a rebind must reach the hint:\n{hint}"
        );
    }
}

// Verifies: gh #398 design point 3 - **the confirmation is transient and shares
// the toast window**.
//
// A permanent "Copied" would keep claiming a state that is no longer true, and a
// *second* timer would drift from the toast's. Both copies of the notice are one
// mechanism, which is what the brief asked for.
#[test]
fn the_confirmation_shares_the_toast_window() {
    let overlays = code_lines(&source("crates/lca-ui/src/chat_overlays.rs"));
    let hint = overlays
        .split("fn auth_url_hint")
        .nth(1)
        .and_then(|rest| rest.split("pub fn note_url_copied").next())
        .expect("auth_url_hint is present");
    assert!(
        hint.contains("TOAST_SECS"),
        "the confirmation expires on the toast's own window:\n{hint}"
    );
    assert!(
        hint.contains("url_copied_at"),
        "and it is driven by the timestamp the copy path sets:\n{hint}"
    );

    let run = code_lines(&source("crates/lca-ui/src/run.rs"));
    assert!(
        run.contains("chat.note_url_copied("),
        "the copy path records the confirmation, so pressing the key is what \
         shows it"
    );
    assert!(
        run.contains("CopyReport::Failed"),
        "and a failed copy never claims success"
    );
}

// Verifies: gh #398 gap 1 - **both wrapped-URL surfaces behave the same way**.
//
// Found live, not in the issue: the `/mcp` sign-in screen shows the *other*
// multi-row authorization URL, and before this cycle `Ctrl+X` there did nothing
// at all - the modal owned the keyboard and the copy key was refused with it. On
// a remote box that screen is the only route to the provider, so it was the same
// defect one surface over.
//
// Two surfaces with two conventions is how a reader learns to trust neither, so
// the guard pins the *shared* rule: one key, one hint builder, one confirmation.
#[test]
fn both_wrapped_url_surfaces_share_the_copy_path() {
    let keys = code_lines(&source("crates/lca-ui/src/chat_keys.rs"));
    assert!(
        keys.contains("fn mcp_sign_in_url"),
        "the MCP sign-in URL is named as a copy target"
    );
    assert!(
        keys.contains("mcp_sign_in_url().is_some()"),
        "and the copy key is let through while that screen owns the keyboard"
    );

    // One hint builder for both surfaces: the MCP renderer takes the hint as a
    // parameter rather than growing its own.
    let overlays = code_lines(&source("crates/lca-ui/src/chat_overlays.rs"));
    assert!(
        overlays.contains("mcp_sign_in_url().map(|url| self.auth_url_hint(&url))"),
        "the MCP screen uses the login wait's hint builder, not a copy of it"
    );

    let run = code_lines(&source("crates/lca-ui/src/run.rs"));
    assert!(
        run.contains("from_wait || from_mcp"),
        "and the confirmation records for either surface"
    );
}

// Verifies: gh #398 gap 1 - **the hint sits above the URL on the MCP screen**.
//
// Found live in a 30-row pane: the MCP sign-in body is windowed (`▼ N more`) and
// a five-row wrapped URL fills the visible rows, so a hint *below* the URL was
// the first thing scrolled out - invisible exactly when a reader needed it. The
// row is placed above.
#[test]
fn the_mcp_hint_is_placed_above_the_wrapped_url() {
    let render = code_lines(&source("crates/lca-ui/src/chat_mcp_render.rs"));
    let signin = render
        .split("fn signin_screen(")
        .nth(1)
        .and_then(|rest| rest.split("/// The first").next())
        .expect("signin_screen is present");
    let hint_at = signin.find(r#"format!("  {hint}")"#);
    let url_at = signin.find(r#"format!("  {url}")"#);
    assert!(
        hint_at.is_some() && url_at.is_some(),
        "both rows are drawn:\n{signin}"
    );
    assert!(
        hint_at < url_at,
        "the hint comes before the URL, so the window cannot scroll it away"
    );
}

// Verifies: gh #398's secrets-hygiene constraint - **the copied URL is never
// logged**.
//
// The URL is not itself a secret, but the redirect it leads to is: an
// authorization code rides in it. Nothing in the copy path may write the value
// anywhere but the clipboard.
#[test]
fn the_copy_path_never_logs_the_url() {
    let run = code_lines(&source("crates/lca-ui/src/run.rs"));
    let section = run
        .split("chat.message_copy_key(data)")
        .nth(1)
        .and_then(|rest| rest.split("chat.alt_screen_bottom(data)").next())
        .expect("the copy section of the input loop is present");
    for sink in ["eprintln!", "println!", "dbg!", "log::", "tracing::"] {
        assert!(
            !section.contains(sink),
            "the copy path writes nothing to a log (`{sink}`)"
        );
    }
}

// ---------------------------------------------------------------------------
// Gh #398 gap 2: a multi-step login keeps its trail.
// ---------------------------------------------------------------------------
//
// # The defect
//
// Each login step used to **clear** the previous one (`login_step.rs`: "One step
// owns the screen at a time: entering one clears the others"). In a URL -> code
// -> workspace flow that meant that by the time a login failed, the URL the user
// had been told to visit and the code they had been told to paste were both
// gone - which is precisely the information needed to work out which step went
// wrong. pi keeps the trail (`showPrompt` appends; `replaceInputWithSubmittedText`
// swaps the answered input for a static `> value` line).
//
// # The rule, and where it lives
//
// 1. A completed step becomes **history** and the next prompt appears beneath it,
//    so the modal reads as a transcript of the flow.
// 2. A submitted value is displayed as **input** (`> value`), not as prose, so a
//    reader can tell what they typed from what the provider said.
// 3. **A secret typed into a step must not become history in the clear.** This is
//    the risk the fix introduces, so the masked row is written first in
//    `crates/lca-ui/src/chat_login_trail_tests.rs` - and the *decision* lives in
//    one place: `login_step.rs`'s `record_login_answer` records asterisks and
//    stores `None` as the plain text, so the secret is **absent from the entry**
//    rather than redacted at paint time.
// 4. The trail survives a failure.
// 5. The newest prompt stays visible.
// 6. The consent/countdown surface is untouched.
//
// The behaviour rows are beside the code (`chat_login_trail_tests.rs`), because
// they need the private `chat()` fixture. This file guards the properties a
// behaviour row cannot see: that the trail is produced at the one seam all steps
// pass through, and that the masking decision is the *storage* rule rather than a
// render-time one.

/// **A step cannot be cleared without being recorded.** The single place a step
/// enters the interface promotes the step it replaces, so no future call site can
/// clear a step and forget the trail.
#[test]
fn every_step_enters_through_the_one_seam_that_records_it() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/login_step.rs"
    ))
    .expect("the login step module is readable");
    let body = source
        .split("pub fn apply_login_next")
        .nth(1)
        .expect("apply_login_next exists");
    let promotion = body
        .find("promote_pending_step(state);")
        .expect("a step is promoted");
    let first_arm = body.find("match next {").expect("the step match");
    assert!(
        promotion < first_arm,
        "the replaced step is recorded *before* the new one is applied, so a \
         step cannot clear another without becoming history"
    );
}

/// **A secret never becomes history.** The decision is a storage rule: the answer
/// is recorded as asterisks with no plain-text copy, so no later renderer, log,
/// or scrollback change can leak a value that was never stored. A render-time
/// redaction would be one `if` away from leaking.
#[test]
fn a_masked_answer_stores_asterisks_and_no_plain_text() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/login_step.rs"
    ))
    .expect("the login step module is readable");
    let recorder = source
        .split("pub fn record_login_answer")
        .nth(1)
        .expect("record_login_answer exists");
    let masked = recorder
        .find("\"*\".repeat(")
        .expect("a masked answer is asterisks");
    let plain = recorder
        .find("plain: (!prompt.masked).then(")
        .expect("the plain text is stored only when unmasked");
    assert!(
        masked < plain,
        "the masked branch decides the stored value, not the painter"
    );
}

/// The trail renders through the same overlay layout as the modal, so a
/// wrap-width disagreement cannot put the live prompt under the fold as an
/// overflow rather than as a failing row.
#[test]
fn the_trail_measures_with_the_painters_own_layout() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ui/src/login_trail.rs"
    ))
    .expect("the trail module is readable");
    assert!(
        source.contains("resolve_overlay_layout"),
        "the trail derives its budget from the layout the painter resolves, not \
         from a second estimate of the same quantity"
    );
    assert!(
        source.contains("wrap_text_with_ansi(&linkify_urls("),
        "and measures rows wrapped, because a sign-in URL is three or four \
         painted rows"
    );
}
