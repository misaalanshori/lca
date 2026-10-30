//! Gh #301: the reference `ui` extension must not leak a pty child or erase its
//! own panel.
//!
//! Both defects lived in `extensions/ui-example/src/lib.rs`, and both were the
//! same kind of mistake: a rule that was **true of the code as written** and
//! false of the behaviour. `on_session_close` existed, was correct, and was never
//! reached for a `ui`-only extension; `render` drained a chunk and drew it, which
//! is right whenever a chunk arrives and blank the rest of the time.
//!
//! The behavioural rows live in `extensions/ui-example/tests/panel_lifecycle.rs`
//! and `extensions/ui-example/src/panel_screen.rs`. This file is the NFR-24
//! guard, and it covers the two things a behavioural row cannot see because it
//! measures the *current* code:
//!
//! 1. **a second net exists at all** — `Drop` for the abnormal path, and the
//!    teardown it calls is idempotent, because two nets that both fire is the
//!    ordinary exit and a double kill must not be an error;
//! 2. **the panel draws from kept state, not from whatever this frame read** —
//!    the shape that made the panel blank, restored, must be visible as a
//!    deletion of the screen, not as a passing test.

use std::path::PathBuf;

/// A source file from the workspace root.
fn source(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// The body of `fn <name>` in `src`, from its `{` to the matching `}`.
///
/// Brace counting is enough here because the functions this file inspects hold no
/// braces inside string literals - and a guard that cannot find its function
/// fails loudly rather than passing vacuously.
fn function_body(src: &str, name: &str) -> String {
    // The name must end here: without the check, `render` would match
    // `fn render_region` and the guard would inspect the wrong function with no
    // sign that it had. A `(` or a `<` is what a real declaration puts next.
    let needle = format!("fn {name}");
    let start = src
        .match_indices(&needle)
        .find(|(at, _)| matches!(src[at + needle.len()..].chars().next(), Some('(' | '<')))
        .map(|(at, _)| at)
        .unwrap_or_else(|| panic!("`{name}` is declared:\n{src}"));
    let open = start
        + src[start..]
            .find('{')
            .unwrap_or_else(|| panic!("`{name}` has a body"));
    let mut depth = 0usize;
    let mut end = open;
    for (offset, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(end > open, "`{name}`'s body is closed");
    src[open..=end].to_string()
}

// Verifies: the abnormal path is covered, and - corrected by gh #578 - that it is
// the **only** path this extension has.
//
// #301's first version of this row asserted that `on_session_close` *and* `Drop`
// both ended the session. `Drop` does; the other half asserted the behaviour of a
// method the host never dispatches, because `on-session-close` is a `hooks`-world
// export and this extension declares `ui` alone. A row that calls a dead hook
// directly proves the hook works and says nothing about whether it is reachable -
// which is the defect #578 exists for. The assertion is now the honest one: `Drop`
// is the net that runs, and there is no second net to keep in step.
#[test]
fn the_reference_extension_tears_down_in_drop() {
    let src = source("extensions/ui-example/src/lib.rs");
    let drop_body = function_body(&src, "drop");
    assert!(
        drop_body.contains("end_panel_session"),
        "`Drop` is the net that always runs and must end the session:\n{drop_body}"
    );
    assert!(
        !src.contains("fn on_session_close"),
        "and the extension implements no hook, because it cannot receive one \
         (gh #578): an `on_session_close` here would be dead code"
    );
}

// Verifies: both nets call **one** teardown, and that teardown takes the handle
// out before acting on it. That `take()` is what makes a double kill safe; a
// `get()` followed by a kill would issue `pty_kill` twice and the second is an
// error on a released handle.
#[test]
fn the_two_nets_share_one_idempotent_teardown() {
    let src = source("extensions/ui-example/src/lib.rs");
    let body = function_body(&src, "end_panel_session");
    assert!(
        body.contains(".take()"),
        "the teardown removes the handle before killing it, so a second call is a \
         no-op rather than a second kill:\n{body}"
    );
    assert!(
        body.contains("pty_kill"),
        "and it is what actually kills the child:\n{body}"
    );
}

// Verifies: the render path folds every read into kept state and then draws
// **that**, rather than drawing only what this frame happened to read. A render
// that returns early on an empty chunk is exactly the defect; matched here on the
// `panel_nodes(.., Some(&screen))` call, because a tree built without the screen
// cannot show it.
#[test]
fn the_panel_is_drawn_from_kept_state_not_from_this_frame_s_read() {
    let src = source("extensions/ui-example/src/lib.rs");
    let body = function_body(&src, "render");
    assert!(
        body.contains("screen.observe"),
        "every panel frame feeds the rolling screen:\n{body}"
    );
    assert!(
        body.contains("panel_nodes(session.as_ref(), Some(&screen))"),
        "and draws the screen, not the chunk:\n{body}"
    );
    assert!(
        !body.contains("Ok(Some(_)) => return"),
        "an empty chunk must not short-circuit the draw:\n{body}"
    );
}

// Verifies: the rolling screen is **bounded**. Both bounds are the decision the
// brief asked for, and a fast producer is the adversarial case - a screen with no
// bound is a memory leak wearing the fix's clothes.
#[test]
fn the_rolling_screen_is_bounded_on_both_axes() {
    let src = source("extensions/ui-example/src/panel_screen.rs");
    assert!(
        src.contains("SCREEN_MAX_BYTES") && src.contains("SCREEN_MAX_ROWS"),
        "the screen keeps a byte bound and a row bound"
    );
    let trim = function_body(&src, "trim");
    assert!(
        trim.contains("SCREEN_MAX_ROWS") && trim.contains("SCREEN_MAX_BYTES"),
        "and trimming enforces both:\n{trim}"
    );
}

// Verifies: trimming lands on a line boundary and an escape sequence is never
// cut in half - the two rendering hazards the brief named, and the pair that
// shows as visible corruption when wrong.
#[test]
fn trimming_is_line_safe_and_escape_safe() {
    let src = source("extensions/ui-example/src/panel_screen.rs");
    let cut = function_body(&src, "safe_cut");
    assert!(
        cut.contains("find('\\n')"),
        "the cut is placed at a newline so a row is never half-kept:\n{cut}"
    );
    assert!(
        cut.contains("escape_boundary"),
        "and a sequence the cut lands inside is dropped whole:\n{cut}"
    );
    let rows = function_body(&src, "rows");
    assert!(
        rows.contains("strip_ansi") || rows.contains("self.text()"),
        "rows are drawn from stripped text, never raw bytes:\n{rows}"
    );
}

// Verifies: a dead child is visible rather than silently frozen - the honesty
// item. The exit code is recorded and the panel renders a label from it.
#[test]
fn an_exited_session_is_reported_rather_than_frozen() {
    let src = source("extensions/ui-example/src/panel_screen.rs");
    assert!(
        src.contains("fn ended_label"),
        "the panel has a word for a session that ended"
    );
    let observer = function_body(&src, "observe");
    assert!(
        observer.contains("self.ended"),
        "and the exit code is recorded when the read reports it:\n{observer}"
    );
    let nodes = source("extensions/ui-example/src/lib.rs");
    assert!(
        nodes.contains("PanelScreen::ended"),
        "the panel tree asks whether the session ended"
    );
}
