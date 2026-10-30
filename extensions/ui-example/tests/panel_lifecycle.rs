//! The panel's two teardown nets, and the panel as a *screen* rather than a
//! stream (gh #301).
//!
//! Two things are pinned here that only a live pty can show:
//!
//! - **the child is a real process**, so "the session ended" is a claim to check
//!   against the process table, not against a flag the extension set;
//! - **a frame with no new bytes still draws**, which is the defect the issue
//!   filed - a unit row on the screen alone would not prove the *render* path
//!   re-draws it.
//!
//! Every row drives the real `UiExample` against a real `sh`/`cmd.exe` in a real
//! scratch sandbox. Where a row can be faked it is not: a fake pty would pass
//! with the leak and the blank frame both present.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_ext_abi::ExtensionDispatch;
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};

struct Always;
impl PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn sandbox(name: &str) -> Arc<lca_tools::Capabilities> {
    let root = lca_testkit::scratch_path(&format!("ui-life-{name}"));
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let roots = ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    };
    Arc::new(lca_tools::Capabilities::new(
        "ui-example",
        ui_example::manifest_grants(),
        roots,
        Arc::new(Mutex::new(Always)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    ))
}

/// The panel as text, the way the host would draw it.
fn panel_lines(handle: &dyn ExtensionDispatch) -> String {
    let tree = handle
        .render("panel")
        .expect("render")
        .expect("the panel has content");
    let theme = lca_ui::theme::Theme::colored();
    lca_ui::ext_widgets::widget_lines(
        &tree.nodes,
        &lca_ui::ext_widgets::WidgetCtx {
            theme: &theme,
            width: 60,
            region: "panel",
            offsets: &std::collections::HashMap::new(),
        },
    )
    .join("\n")
}

/// Whether the program has printed a prompt (or anything at all).
///
/// Windows' ConPTY takes noticeably longer to deliver its first bytes than a
/// unix pty does, and `cmd.exe` prints a banner before its prompt. Typing before
/// any of that arrives is not a defect - the write is buffered and the shell
/// runs the line - but it makes a fixed sleep flaky, so every row that types
/// waits for the program to speak first.
fn program_has_spoken(handle: &dyn ExtensionDispatch) -> bool {
    let header = ["session:", "keys:", "not available"];
    panel_lines(handle).lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !header.iter().any(|h| line.starts_with(h))
    })
}

/// Type one line and wait for it to have any visible effect.
fn type_line(handle: &dyn ExtensionDispatch, line: &str) {
    // Start the session first: opening the panel is the invocation, and a
    // keystroke sent before it lands on nothing (the extension has no handle to
    // write to yet).
    let _ = panel_lines(handle);
    for _attempt in 0..80 {
        if program_has_spoken(handle) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    handle
        .on_ui_event(
            "panel",
            &lca_protocol::UiInput::Key {
                key: format!("{line}\r"),
            },
        )
        .expect("event");
}

/// Type a line into the panel and wait for the program to echo it back.
fn type_and_wait(handle: &dyn ExtensionDispatch, line: &str) -> String {
    type_line(handle, line);
    let mut panel = String::new();
    for _attempt in 0..80 {
        panel = panel_lines(handle);
        // The echoed command and the program's own output both contain the
        // marker; two occurrences means the shell has run it, not merely echoed
        // the keystroke back at the terminal.
        if panel.matches(line).count() >= 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panel
}

/// Drive a future that is already resolved, without pulling in a runtime crate.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    // A `Box::pin(std::future::ready(..))` answers `Ready` on the first poll; a
    // future that did not would be a defect in the extension, and the panic
    // names it rather than hanging the suite.
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(out) => out,
        Poll::Pending => panic!("the dispatch future resolved without a runtime"),
    }
}

/// Whether a pty handle is still in the engine's table - the only honest way to
/// ask "did the kill happen" without inventing a flag.
fn handle_alive(cap: &lca_tools::Capabilities, handle: u32) -> bool {
    // A live handle answers a resize; a released one is `NotFound`.
    cap.pty_resize(handle, 24, 80).is_ok()
}

// Verifies: gh #301 defect 2, end to end through `render` - a frame drawn when
// the session produced nothing new still shows the output that is already there.
// This is the erasure the issue filed; the row fails on the pre-fix shape, where
// the read's `Ok(Some(empty))` left nothing to draw.
#[test]
fn a_frame_with_no_new_output_still_shows_the_previous_output() {
    let cap = sandbox("persist");
    let handle = ui_example::UiExample::new(cap.clone());
    let panel = type_and_wait(&handle, "echo PERSIST301");
    assert!(
        panel.contains("PERSIST301"),
        "the program answered: {panel}"
    );

    // Now draw several frames with the program silent: no keystrokes, so nothing
    // new can arrive. Every one of them must still show the output.
    for frame in 0..5 {
        let again = panel_lines(&handle);
        assert!(
            again.contains("PERSIST301"),
            "frame {frame} erased the panel: {again:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}

// Verifies: a frame *with* new bytes appends and shows the tail - the other half
// of the same rule, so "keep everything" cannot pass for "shows the newest".
#[test]
fn a_frame_with_new_output_appends_and_shows_the_tail() {
    let cap = sandbox("append");
    let handle = ui_example::UiExample::new(cap.clone());
    let first = type_and_wait(&handle, "echo FIRST301");
    assert!(first.contains("FIRST301"), "{first}");
    let second = type_and_wait(&handle, "echo SECOND301");
    assert!(second.contains("SECOND301"), "{second}");
    assert!(
        second.contains("FIRST301"),
        "the earlier output is still on the screen: {second}"
    );
}

// Verifies: `on_session_close` kills the child. Asserted against the engine's
// handle table, not against a field the extension set, because a flag would pass
// with the process still running.
#[test]
fn on_session_close_ends_the_panel_session() {
    let cap = sandbox("close");
    let handle = ui_example::UiExample::new(cap.clone());
    let _ = panel_lines(&handle);
    let session = handle.panel_session().expect("session started");
    assert!(handle_alive(&cap, session), "precondition: the pty is live");

    // The dispatch future is a `Box::pin`ed ready future, so a test can await it
    // without a runtime; `poll` once is enough and says so.
    block_on(handle.on_session_close()).expect("session close");

    assert!(!handle_alive(&cap, session), "the pty handle was released");
    assert_eq!(handle.panel_session(), None, "and the extension forgot it");
}

// Verifies: `Drop` kills the child too - the abnormal path, where no session
// close is ever dispatched. The engine is dropped *after* the handle, so this is
// the exact ordering that leaked: the extension is gone and its `sh` is not.
#[test]
fn dropping_the_handle_ends_the_panel_session() {
    let cap = sandbox("drop");
    let session;
    {
        let handle = ui_example::UiExample::new(cap.clone());
        let _ = panel_lines(&handle);
        session = handle.panel_session().expect("session started");
        assert!(handle_alive(&cap, session), "precondition: the pty is live");
    } // `handle` dropped here; `cap` is still alive, which is the real case.

    assert!(
        !handle_alive(&cap, session),
        "the pty is released even though the engine outlived the extension"
    );
}

// Verifies: the two nets do not fight. A close followed by a drop is the ordinary
// exit path - `session_close` then the registry dropping - and the second kill
// must be a no-op rather than an error or a second kill of a reused handle.
#[test]
fn a_double_kill_is_safe() {
    let cap = sandbox("double");
    let session;
    {
        let handle = ui_example::UiExample::new(cap.clone());
        let _ = panel_lines(&handle);
        session = handle.panel_session().expect("session started");
        // First net.
        assert_eq!(handle.end_panel_session(), Some(session));
        // Second net, explicitly, then again by `Drop` when this scope ends.
        assert_eq!(
            handle.end_panel_session(),
            None,
            "the second call is a no-op"
        );
        block_on(handle.on_session_close()).expect("session close");
        assert_eq!(handle.end_panel_session(), None);
    }
    assert!(!handle_alive(&cap, session));
    // A fresh session still starts afterwards: the handle table was not poisoned
    // by the redundant calls, and ids are not reused while an entry exists.
    let fresh = ui_example::UiExample::new(cap.clone());
    let _ = panel_lines(&fresh);
    assert!(fresh.panel_session().is_some(), "the demo still starts");
}

// Verifies: a dead child is visible rather than frozen on its last output. The
// program is told to exit, and the panel says so - on its own, with no keystroke
// needed and no blank column.
#[test]
fn an_exited_session_says_so_instead_of_freezing() {
    let cap = sandbox("ended");
    let handle = ui_example::UiExample::new(cap.clone());
    let _ = panel_lines(&handle);
    // Exit and leave a last line behind: the panel must show both the output and
    // the fact that the session is over.
    //
    // The two shells' separators differ - `cmd.exe` reads `;` as part of the
    // argument and echoed the whole line literally on Windows CI, so `&` is the
    // one that runs both commands there. Written per-platform rather than as one
    // guess, because the row is about the *panel*, not about shell syntax.
    let line = if cfg!(windows) {
        "echo BYE301 & exit"
    } else {
        "echo BYE301; exit"
    };
    let _ = type_and_wait(&handle, line);
    let mut panel = String::new();
    for _attempt in 0..80 {
        panel = panel_lines(&handle);
        if panel.contains("BYE301") && panel.contains("ended") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        panel.contains("BYE301"),
        "the last output survives: {panel}"
    );
    assert!(
        panel.contains("session ended"),
        "the panel says the session is over: {panel}"
    );
    assert!(
        !panel.contains("type here:"),
        "and stops inviting input: {panel}"
    );
}

// Verifies: the panel is a screen the host draws, so nothing the program emits
// can reach the terminal as a sequence - gh #301's third finding, where escape
// debris showed as visible `\x1b[31m` text.
#[test]
fn escape_sequences_from_the_program_never_reach_the_row() {
    let cap = sandbox("ansi");
    let handle = ui_example::UiExample::new(cap.clone());
    let _ = panel_lines(&handle);
    // A red line, in the shell's own printf so the bytes really are raw SGR.
    let _echo = type_and_wait(&handle, "printf '\\033[31mRED301\\033[0m plain301\\n'");
    // Wait for the *output*, not the echoed command: the echo necessarily
    // contains the escape as typed text, and asserting on it would say nothing
    // about what the program emitted.
    let mut panel = String::new();
    for _attempt in 0..80 {
        panel = panel_lines(&handle);
        if panel.matches("RED301").count() >= 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let drawn: Vec<&str> = panel
        .lines()
        .filter(|line| line.contains("RED301") && !line.contains("printf"))
        .collect();
    assert!(
        !drawn.is_empty(),
        "the program's line reached the panel: {panel:?}"
    );
    for row in drawn {
        assert!(
            !row.contains('\u{1b}'),
            "no raw sequence reaches the row: {row:?}"
        );
        // The host's `sanitize_text` renders a surviving control character as
        // visible `\xNN`; that debris is what "the panel shows garbage" means.
        assert!(
            !row.contains("\\x1b"),
            "and no escape debris either - the sanitizer's visible form: {row:?}"
        );
        assert!(
            row.contains("plain301"),
            "the visible text is kept: {row:?}"
        );
    }
}
