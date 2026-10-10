//! End-to-end terminal tests: the real-terminal tmux paths and the login pickers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
#[cfg(any(unix, windows))]
#[cfg(unix)]
#[test]
fn the_tui_renders_a_turn_in_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    // An out-of-workspace path, so the folder-trust auto-approval (ADR-0039)
    // does not skip the modal this test exercises.
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("shell", r#"{"command":"cat /etc/hostname"}"#)),
        Reply::Sse(sse_text_with_usage("turn complete", 20, 0)),
    ]));
    let sandbox = sandbox("tui-smoke");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("turn");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);

    // 1. Startup renders the frame with the configured model.
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    // 2. A prompt asks the permission question for the shell call.
    session.send(&["do a thing", "Enter"]);
    session.wait_for("Allow this action?", std::time::Duration::from_secs(25));

    // 3. Answering it lets the turn complete and render the reply.
    session.send(&["o"]);
    session.wait_for("turn complete", std::time::Duration::from_secs(25));

    // 4. A resize re-renders without losing the frame.
    let resized = session.resize(100, 30);
    assert!(
        resized.contains("openai-compatible") || resized.contains("turn complete"),
        "the frame survives a resize:\n{resized}"
    );

    // 5. A clean quit writes the session-end marker.
    session.send(&["/exit", "Enter"]);
    let log = wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
    assert!(log.contains("\"t\":\"session-end\""), "session-end written");
}

// Verifies: the real-terminal checklist's secret-prompt case - what the user
// types into `/login` never reaches the visible frame.
#[cfg(unix)]
#[test]
fn the_logins_secret_prompt_masks_input_in_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("tui-login-mask");
    let session = Tmux::new("mask");
    // No key: `/login <preset>` skips the picker and asks for the secret.
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));

    session.send(&["/login openrouter", "Enter"]);
    session.wait_for("input hidden", std::time::Duration::from_secs(15));
    let secret = "sk-super-secret-value";
    session.send(&[secret]);
    // Give the frame a beat to render; the secret must not be visible.
    std::thread::sleep(std::time::Duration::from_millis(600));
    let pane = session.capture();
    assert!(
        !pane.contains(secret),
        "the secret never appears in the frame:\n{pane}"
    );
    assert!(pane.contains("input hidden"), "still the masked prompt");
}

// Verifies: R1/R6 - a genuine tmux paste (bracketed, through tmux's own
// paste machinery) reaches the prompt editor. `send-keys` cannot prove
// this: it synthesizes key events, which is typing by another name.
#[cfg(unix)]
#[test]
fn a_real_paste_reaches_the_prompt_editor() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("paste-editor");
    let session = Tmux::new("paste-editor");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));
    session.paste_text("pasted-into-the-editor");
    session.wait_for("pasted-into-the-editor", std::time::Duration::from_secs(20));
}

// Verifies: R1 - a real multi-line paste (>10 lines) becomes an editor
// marker, and the marker is what the frame shows (the content is held in
// the paste registry and expanded at submit).
#[cfg(unix)]
#[test]
fn a_real_multi_line_paste_becomes_a_marker() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("paste-marker");
    let session = Tmux::new("paste-marker");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));
    let big: String = (0..12)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    session.paste_text(&big);
    session.wait_for("[paste #1", std::time::Duration::from_secs(20));
}

// Verifies: R1/R6 - a genuine paste into the masked secret field lands in
// the buffer and is masked; the pasted bytes never reach the frame.
#[cfg(unix)]
#[test]
fn a_real_paste_into_the_secret_field_is_masked() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("paste-secret");
    let session = Tmux::new("paste-secret");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));
    session.send(&["/login openrouter", "Enter"]);
    session.wait_for("input hidden", std::time::Duration::from_secs(15));
    let secret = "sk-pasted-secret-value";
    session.paste_text(secret);
    std::thread::sleep(std::time::Duration::from_millis(700));
    let pane = session.capture();
    assert!(
        !pane.contains(secret),
        "the pasted secret never appears in the frame:\n{pane}"
    );
    assert!(pane.contains("input hidden"), "still the masked prompt");
    // The masked buffer grew, so the asterisks are present (the field is
    // not silently empty after the paste).
    assert!(
        pane.contains("********"),
        "the paste is masked, not dropped:\n{pane}"
    );
}

// Verifies: R1 - a genuine paste into the (unmasked) base-URL field is
// shown as typed, so a long endpoint URL need not be typed by hand.
#[cfg(unix)]
#[test]
fn a_real_paste_into_the_base_url_field_is_shown() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("paste-baseurl");
    let session = Tmux::new("paste-baseurl");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));
    session.send(&["/login", "Enter"]);
    session.wait_for("Sign in with", std::time::Duration::from_secs(15));
    // Walk to the extension-declared custom entry (last row).
    let mut reached = false;
    for _ in 0..24 {
        session.send(&["Down"]);
        std::thread::sleep(std::time::Duration::from_millis(120));
        if session.capture().contains("Custom endpoint") {
            reached = true;
            break;
        }
    }
    assert!(reached, "the walk reached the custom endpoint entry");
    session.send(&["Enter"]);
    session.wait_for("Base URL", std::time::Duration::from_secs(15));
    session.paste_text("https://pasted.example.com/v1");
    session.wait_for("pasted.example.com", std::time::Duration::from_secs(20));
}

// Verifies: R6/R8 - raw bytes injected in fragments (an escape sequence
// split across writes) are reassembled, never dropped: the pasted text
// comes out the other side.
#[cfg(unix)]
#[test]
fn a_fragmented_raw_paste_reassembles() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("paste-fragmented");
    let session = Tmux::new("paste-fragmented");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));
    session.send_raw_fragmented(&["\x1b[200~", "frag", "mented", "\x1b[201~"]);
    session.wait_for("fragmented", std::time::Duration::from_secs(20));
}

// Verifies: ADR-0033 / `api-key-login-plan.md` D1 - `/login` with no
// argument is a list picker: the extension's presets, and always the host's
// declared "Custom endpoint..." entry. The list is longer than the box, so
// it scrolls - and the custom entry, which sits last, has to stay
// reachable.
#[cfg(unix)]
#[test]
fn the_login_picker_lists_the_presets_and_scrolls_to_the_custom_entry() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("tui-login-picker");
    let session = Tmux::new("picker");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));

    session.send(&["/login", "Enter"]);
    session.wait_for("Sign in with", std::time::Duration::from_secs(15));
    let pane = session.capture();
    assert!(pane.contains("OpenRouter"), "a preset is listed:\n{pane}");
    assert!(pane.contains("OpenAI"), "and another:\n{pane}");
    assert!(pane.contains("> "), "the cursor marks a row:\n{pane}");
    assert!(
        !pane.contains("Custom endpoint"),
        "the custom entry is last, below the fold at the top:\n{pane}"
    );

    // Walking to the end reaches it.
    let mut reached = false;
    for _ in 0..24 {
        session.send(&["Down"]);
        std::thread::sleep(std::time::Duration::from_millis(120));
        if session.capture().contains("Custom endpoint") {
            reached = true;
            break;
        }
    }
    assert!(reached, "the declared custom entry is reachable");
}

// Verifies: ADR-0033 - choosing a preset reaches that preset's own field
// prompt, and the picker closes behind it.
#[cfg(unix)]
#[test]
fn a_preset_choice_reaches_its_own_field_prompt() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("tui-login-choose");
    let session = Tmux::new("choose");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));

    session.send(&["/login", "Enter"]);
    session.wait_for("Sign in with", std::time::Duration::from_secs(15));
    // The first row is a bearer preset, so it asks for its key next.
    session.send(&["Enter"]);
    session.wait_for("input hidden", std::time::Duration::from_secs(15));
    let pane = session.capture();
    assert!(
        !pane.contains("Sign in with"),
        "the picker closed when the row was chosen:\n{pane}"
    );
    assert!(pane.contains("API key"), "its own field prompt:\n{pane}");
}

// Verifies: D3 - a local `auth = "none"` preset has no key step, so
// choosing one never opens a secret prompt.
#[cfg(unix)]
#[test]
fn the_picker_moves_to_a_local_preset_that_needs_no_key() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("tui-login-local");
    let session = Tmux::new("local");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));

    session.send(&["/login", "Enter"]);
    session.wait_for("Sign in with", std::time::Duration::from_secs(15));
    // The list is longer than the box, so it scrolls; walk down until the
    // hint names a local host rather than counting rows. CI runners are
    // slow and can drop a redraw behind the capture, so each step gets a
    // short poll window before the next one.
    let mut reached = false;
    'walk: for _ in 0..30 {
        session.send(&["Down"]);
        for _ in 0..4 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if session.capture().contains("localhost") {
                reached = true;
                break 'walk;
            }
        }
    }
    assert!(reached, "the walk reached a local preset");
    // The selection is on the local preset now; let the redraw settle so
    // the Enter below lands on the chosen row.
    std::thread::sleep(std::time::Duration::from_millis(300));
    // A local preset has no key step: Enter signs in without one.
    session.send(&["Enter"]);
    std::thread::sleep(std::time::Duration::from_millis(600));
    let pane = session.capture();
    assert!(!pane.contains("Sign in with"), "the picker closed:\n{pane}");
    assert!(
        !pane.contains("input hidden"),
        "no key was asked for:\n{pane}"
    );
}

// Verifies: ADR-0029 - the interface's `/attach` stages an image and the next
// user message carries its hash and stub.
#[cfg(unix)]
#[test]
fn attach_in_the_tui_stages_an_image_for_the_next_message() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("seen"))]));
    let sandbox = sandbox("tui-attach");
    sandbox.approve_loopback_net(serde_json::json!({}));
    let png = sandbox.project().join("shot.png");
    std::fs::write(
        &png,
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3],
    )
    .expect("write image");

    let session = Tmux::new("attach");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    let attach_command = format!("/attach {}", png.display());
    session.send(&[attach_command.as_str(), "Enter"]);
    session.wait_for("attached", std::time::Duration::from_secs(15));
    session.send(&["look at this", "Enter"]);
    session.wait_for("seen", std::time::Duration::from_secs(25));
    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));

    let log = find_session_log(&sandbox.state_dir()).expect("a session log");
    let text = std::fs::read_to_string(log).expect("read log");
    assert!(
        text.contains("\"attachments\":[\""),
        "the staged hash is on the user record: {text}"
    );
    assert!(
        text.contains("[image attachment"),
        "the stub is in the message text: {text}"
    );
}

// Verifies: the SRDD's restart exit test, FR-SESS-4/FR-SESS-5, and
// FR-CACHE-5/FR-CACHE-6 across a process boundary: a session created in one
// process is resumed in another, the resumed run crosses the compaction
// threshold, the compaction record's replaced range ends before the resumed
// turn (the in-process analogue is
// `after_compaction_a_new_turn_stays_outside_the_stable_prefix`), and the
// clean quit closes the resumed session with its own `session-end`.
#[cfg(unix)]
#[test]
fn a_resumed_session_compacts_at_the_turn_boundary() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text_with_usage("first reply", 50, 0)),
        Reply::Sse(sse_text_with_usage("compaction summary", 5, 0)),
        Reply::Sse(sse_text_with_usage("second reply", 30, 0)),
    ]));
    let sandbox = sandbox("resume-compact");
    // Documented knobs only: `compaction.threshold` is a configuration key
    // (`LCA_COMPACTION_THRESHOLD`), and the provider's window is
    // `OPENAI_CONTEXT_WINDOW` (docs/providers/openai-compatible.md). A
    // 1000-token window at 1% compacts once a turn reports 10 prompt tokens.
    // (gh #36 phase 1: the two-record session fits the default
    // keep-recent window whole, so the test keeps nothing and the
    // candidate is the pre-turn range, as the stopgap saw it.)
    let budget = [
        ("OPENAI_CONTEXT_WINDOW", "1000"),
        ("LCA_COMPACTION_THRESHOLD", "0.01"),
        ("LCA_COMPACTION_KEEP_RECENT_TOKENS", "0"),
    ];

    // Run 1: create the session in one process, then exit.
    let output = sandbox.run_env(Some(&mock), &["-p", "first turn"], &budget);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("first reply"),
        "{}",
        stdout(&output)
    );

    // The session outlives the process.
    let log = find_session_log(&sandbox.state_dir()).expect("a session log");
    let id = log
        .parent()
        .expect("session directory")
        .file_name()
        .expect("session id")
        .to_string_lossy()
        .to_string();

    // Run 2: resume it in a real terminal; the transcript loads from the log
    // before compaction runs.
    let session = Tmux::new("resume");
    session.spawn(&sandbox, Some(&mock), true, &budget, &["resume", &id]);
    session.wait_for("first reply", std::time::Duration::from_secs(25));
    session.send(&["second turn", "Enter"]);
    session.wait_for("second reply", std::time::Duration::from_secs(30));
    session.send(&["/exit", "Enter"]);
    // The resumed process writes its own clean-exit marker before it goes
    // away; run 1 already wrote one, so the log now holds two (FR-SESS-6's
    // clean-exit shape across the restart).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.matches("\"t\":\"session-end\"").count() >= 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the resumed run did not write its own session-end"
        );
        std::thread::sleep(std::time::Duration::from_millis(150));
    }

    // The compaction record lands at the boundary: its replaced range starts
    // at the first turn's user record and ends before the resumed turn, so
    // the resumed turn stays outside the cached prefix.
    let records: Vec<serde_json::Value> = std::fs::read_to_string(&log)
        .expect("read the session log")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("record json"))
        .collect();
    let compaction = records
        .iter()
        .position(|record| record["t"] == "compaction")
        .expect("a compaction record (FR-SESS-4)");
    let resumed_user = records
        .iter()
        .rposition(|record| record["t"] == "user")
        .expect("the resumed turn's user record");
    let first_user = records
        .iter()
        .find(|record| record["t"] == "user")
        .expect("the first turn's user record");
    assert_eq!(
        records[compaction]["replaced_from"], first_user["id"],
        "the replaced range starts at the first turn"
    );
    assert_ne!(
        records[compaction]["replaced_to"], records[resumed_user]["id"],
        "the resumed turn is not inside the replaced range"
    );
    let range_end = records
        .iter()
        .position(|record| record["id"] == records[compaction]["replaced_to"])
        .expect("the replaced range's end is a real record");
    assert!(
        range_end < resumed_user,
        "the replaced range ends before the resumed turn: {records:#?}"
    );
    assert_eq!(records[compaction]["summary"], "compaction summary");
}

// ---------------------------------------------------------------------------
// Real-terminal tests on Windows (docs/testing-plan.md section 14): the TUI
// under a pseudo-console (ConPTY). The checklist is the same as the tmux
// suite; the driver is not.
// ---------------------------------------------------------------------------

/// Read the pseudo-console until `needle` appears in the accumulated screen,
/// or the deadline passes. ConPTY's read is a non-blocking peek, so this
/// polls.
#[cfg(windows)]
fn read_until(
    pty: &mut lca_tools::PtyChild,
    screen: &mut String,
    needle: &str,
    timeout: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if screen.contains(needle) {
            return true;
        }
        if std::time::Instant::now() > deadline {
            return false;
        }
        match pty.read(65536) {
            Ok(Some(chunk)) if !chunk.is_empty() => {
                screen.push_str(&String::from_utf8_lossy(&chunk));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            _ => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

/// The sandbox environment the TUI process needs, as `PtyChild::envs`.
#[cfg(windows)]
fn console_env(sandbox: &Sandbox, mock: &Mock, with_key: bool) -> Vec<(&'static str, String)> {
    let home = sandbox.home.to_string_lossy().into_owned();
    let data = sandbox.data.to_string_lossy().into_owned();
    let config = sandbox.home.join(".config").to_string_lossy().into_owned();
    let mut envs = vec![
        ("HOME", home.clone()),
        ("USERPROFILE", home),
        ("XDG_DATA_HOME", data.clone()),
        ("APPDATA", data.clone()),
        ("LOCALAPPDATA", data.clone()),
        ("XDG_CONFIG_HOME", config),
        ("LCA_UPDATE_CHECK", "false".to_string()),
        ("OPENAI_BASE_URL", mock.url()),
    ];
    if with_key {
        envs.push(("OPENAI_API_KEY", "test-key".to_string()));
    }
    envs
}

#[cfg(windows)]
fn spawn_console(sandbox: &Sandbox, envs: &[(&'static str, String)]) -> lca_tools::PtyChild {
    let borrowed: Vec<(&str, &str)> = envs
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    lca_tools::PtyChild::spawn(
        env!("CARGO_BIN_EXE_lca"),
        &[],
        &sandbox.project(),
        40,
        140,
        &borrowed,
    )
    .expect("spawn the TUI on a ConPTY")
}

/// Serialize the Windows ConPTY tests: each spawns a whole TUI plus a mock
/// server, and running several at once starves them enough to miss the
/// shutdown deadline. One at a time is still fast (each is sub-second).
#[cfg(windows)]
fn conpty_serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

// Verifies: the real-terminal checklist (docs/testing-plan.md section 14) on
// the Windows harness: startup renders, a scripted turn streams and renders,
// the permission modal answers, a resize re-renders, and a clean quit writes
// `session-end`.
#[cfg(windows)]
#[test]
fn the_tui_renders_a_turn_in_a_windows_console() {
    let _serial = conpty_serial();
    let runtime = rt();
    // A review-class shell command (a path outside the workspace) so the
    // analyzer does not auto-approve and the modal is exercised; `type` of a
    // system file is harmless to actually run.
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call(
            "shell",
            r#"{"command":"type C:\\Windows\\win.ini"}"#,
        )),
        Reply::Sse(sse_text_with_usage("turn complete", 20, 0)),
    ]));
    let sandbox = sandbox("tui-conpty");
    sandbox.approve_loopback_net(serde_json::json!({}));

    // The mock needs an explicit model: there is no implicit default
    // (issue #3), so the footer would otherwise read "no model". Matches
    // the tmux harness's `OPENAI_MODEL=test-model`.
    let mut envs = console_env(&sandbox, &mock, true);
    envs.push(("OPENAI_MODEL", "test-model".to_string()));
    let mut pty = spawn_console(&sandbox, &envs);
    let mut screen = String::new();

    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "openai-compatible",
            std::time::Duration::from_secs(60)
        ),
        "startup renders the frame: {screen:?}"
    );
    assert!(
        !screen.contains(r"\\?\"),
        "the verbatim path prefix never reaches the screen: {screen:?}"
    );

    pty.write(b"do a thing\r").expect("write the prompt");
    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "Allow this action?",
            std::time::Duration::from_secs(60)
        ),
        "the permission modal renders: {screen:?}"
    );
    pty.write(b"o").expect("approve once");
    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "turn complete",
            std::time::Duration::from_secs(60)
        ),
        "the turn completes after approval: {screen:?}"
    );

    // A resize re-renders without losing the frame (FR-UI-3).
    pty.resize(30, 100).expect("resize");
    let mut resized = String::new();
    assert!(
        read_until(
            &mut pty,
            &mut resized,
            "openai-compatible",
            std::time::Duration::from_secs(15)
        ),
        "the frame survives a resize: {resized:?}"
    );

    pty.write(b"/exit\r").expect("write /exit");
    let log = find_session_log(&sandbox.state_dir()).expect("a session log");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        // Keep draining the pseudo-console: a TUI writing its shutdown
        // sequences while nobody reads can fill the pipe and block its own
        // exit (and then no session-end is ever written).
        let _ = read_until(
            &mut pty,
            &mut screen,
            "\u{0}",
            std::time::Duration::from_millis(50),
        );
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains("\"t\":\"session-end\"") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "a clean quit writes session-end"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // §14 also asks that the session be resumable: `lca export` reloads the
    // log from disk and replays the turn, which is what resume does.
    let session_id = log
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .expect("the session directory names the session");
    let out = sandbox.run(None, &["export", &session_id]);
    // `lca export` writes the file and prints its path; load and check it.
    let export_path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let exported = std::fs::read_to_string(&export_path).unwrap_or_default();
    assert!(
        exported.contains("do a thing") && exported.contains("turn complete"),
        "the session reloads for resume ({export_path}): {exported}"
    );
}

// Verifies: the real-terminal checklist's secret-prompt case on Windows -
// what the user types into `/login` never reaches the visible screen.
#[cfg(windows)]
#[test]
fn the_logins_secret_prompt_masks_input_in_a_windows_console() {
    let _serial = conpty_serial();
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("tui-conpty-mask");
    sandbox.approve_loopback_net(serde_json::json!({}));

    // No key: `/login` asks for the secret.
    let envs = console_env(&sandbox, &mock, false);
    let mut pty = spawn_console(&sandbox, &envs);
    let mut screen = String::new();

    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "no model",
            std::time::Duration::from_secs(60)
        ),
        "the no-model state renders: {screen:?}"
    );
    // `/login <preset>` skips the picker and asks for the secret directly
    // (ADR-0033); with no argument it would open the list picker first.
    pty.write(b"/login openrouter\r").expect("write /login");
    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "input hidden",
            std::time::Duration::from_secs(30)
        ),
        "the masked prompt renders: {screen:?}"
    );

    let secret = "sk-super-secret-value";
    pty.write(secret.as_bytes()).expect("write the secret");
    std::thread::sleep(std::time::Duration::from_millis(800));
    let mut settle = String::new();
    let _ = read_until(
        &mut pty,
        &mut settle,
        "\u{0}",
        std::time::Duration::from_millis(800),
    );
    screen.push_str(&settle);
    assert!(
        !screen.contains(secret),
        "the secret never appears on the ConPTY screen: {screen:?}"
    );
}

// Verifies: FR-PROV-6's report on the headless path: a script needs a
// loud failure with a nonzero exit, so headless is *not* made recoverable
// along with the interactive surface. "No model" here is the zero-provider
// state - no enabled provider answers the configured name - which is the
// one the fix is about.
#[test]
fn headless_without_an_enabled_provider_still_fails_loud_with_exit_two() {
    let sandbox = sandbox("headless-no-model");
    let out = sandbox.run(None, &["ext", "disable", "openai-compatible"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = sandbox.run(None, &["-p", "hello"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "a script is told, not left hanging: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("No model is available"), "{stderr}");
    assert!(
        stderr.contains("lca ext enable"),
        "the report names the way back: {stderr}"
    );
}

// Verifies: FR-PROV-9 - disabling the only provider must not lock the
// interface out of existence. The design states zero providers is a valid
// state, and the interface is the surface for settings, so it has to be
// able to open into that state and leave it through `/login`.
#[cfg(unix)]
#[test]
fn the_interface_opens_in_the_zero_provider_state_and_recovers_through_login() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("tui-zero-provider");
    // Disable the only provider, from the project the session will run in.
    let out = sandbox.run(None, &["ext", "disable", "openai-compatible"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let session = Tmux::new("zeroprov");
    session.spawn(&sandbox, None, false, &[], &[]);
    // The first frame names the state and the way out.
    session.wait_for("No model is available", std::time::Duration::from_secs(20));
    let pane = session.capture();
    assert!(
        pane.contains("/login"),
        "the first frame says how to recover: {pane}"
    );

    // `/login` with zero providers names the state and the way out
    // (gh #188): the host synthesizes no phantom custom entry, so the
    // message points at `ext install` / `ext enable` instead of a
    // picker that could configure nothing.
    session.send(&["/login", "Enter"]);
    let pane = session.wait_for("no login options", std::time::Duration::from_secs(15));
    assert!(
        pane.contains("lca ext enable"),
        "the message names the way back: {pane}"
    );
    session.send(&["Escape"]);
    std::thread::sleep(std::time::Duration::from_millis(300));
    session.send(&["/exit", "Enter"]);
    // No turns ran, so no log exists for a session-end marker (gh
    // #122): the dead pane is the receipt.
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
    drop(session);

    // Re-enable the provider and come back: the state has to be
    // *leavable*, not merely reportable. The custom endpoint is now
    // the extension's own declared preset, reached the same way.
    let out = sandbox.run(None, &["ext", "enable", "openai-compatible"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let session = Tmux::new("zeroprov-back");
    session.spawn(&sandbox, None, false, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));
    session.send(&["/login", "Enter"]);
    session.wait_for("Sign in with", std::time::Duration::from_secs(15));

    // Walk to the extension-declared custom entry (last row) and
    // complete the three fields.
    for _ in 0..24 {
        session.send(&["Down"]);
        std::thread::sleep(std::time::Duration::from_millis(120));
        if session
            .capture()
            .contains("Custom endpoint\u{2026}   base URL")
            || session.capture().contains("base URL + key + model")
        {
            break;
        }
    }
    session.send(&["Enter"]);
    session.wait_for("Base URL", std::time::Duration::from_secs(15));
    session.send(&["https://example.test/v1", "Enter"]);
    session.wait_for("input hidden", std::time::Duration::from_secs(15));
    session.send(&["sk-x", "Enter"]);
    session.wait_for("Model id", std::time::Duration::from_secs(15));
    session.send(&["m1", "Enter"]);
    // A host outside the manifest's vocabulary gets the ad hoc `net` grant
    // prompt (FR-PERM-16), and answering it is the last step of the flow.
    session.wait_for("ad hoc grant", std::time::Duration::from_secs(15));
    session.send(&["y"]);
    // The confirmation is a transient notice by design, so assert the
    // durable state it left behind rather than a flash of text. Poll it
    // boundedly: a fixed sleep races a slow runner, while the file
    // either carries the key or it does not. The key is canonicalized
    // exactly like the store's own `canonical_key`: on macOS the scratch
    // dir hangs under symlinked `/var`, while the running binary records
    // `/private/var` - a raw join misses every time there.
    let project_key = std::fs::canonicalize(sandbox.project())
        .unwrap_or_else(|_| sandbox.project())
        .to_string_lossy()
        .to_string();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let grants: serde_json::Value = loop {
        let grants: serde_json::Value =
            std::fs::read_to_string(sandbox.state_dir().join("grants.json"))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(serde_json::Value::Null);
        if grants["projects"][&project_key]["extensions"]["openai-compatible"]
            == serde_json::json!(true)
            || std::time::Instant::now() >= deadline
        {
            break grants;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert_eq!(
        grants["projects"][&project_key]["extensions"]["openai-compatible"],
        serde_json::json!(true),
        "the provider is enabled again"
    );
}

// Verifies: ADR-0039's Windows surface end to end - a project carrying an
// untrusted `.lca/config.toml` prompts at startup, the trust answer applies,
// a `!` command runs inline (the user's own, ungated), `/grants` opens, and
// a clean `/exit` quits.
#[cfg(windows)]
#[test]
fn the_trust_prompt_inline_shell_and_grants_work_on_windows() {
    let _serial = conpty_serial();
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("tui-conpty-trust");
    // No seeded grants: the project must be untrusted for the prompt. An
    // untrusted project config is what raises it at startup (FR-PERM-24).
    std::fs::create_dir_all(sandbox.project().join(".lca")).expect("mkdir .lca");
    std::fs::write(sandbox.project().join(".lca").join("config.toml"), "").expect("write config");

    let mut envs = console_env(&sandbox, &mock, false);
    envs.push(("OPENAI_MODEL", "test-model".to_string()));
    // Deliver a lone Escape quickly, so it is not reassembled with the
    // Ctrl+C that follows into an alt-sequence.
    envs.push(("PI_TUI_ESC_TIMEOUT", "50".to_string()));
    let mut pty = spawn_console(&sandbox, &envs);
    let mut screen = String::new();

    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "Trust this project",
            std::time::Duration::from_secs(30)
        ),
        "the folder-trust prompt renders: {screen:?}"
    );
    pty.write(b"\r").expect("trust the folder"); // row 0: trust (remember)

    // `!` runs the user's own command inline, no modal.
    pty.write(b"!echo handdrive-marker\r")
        .expect("write ! command");
    let mut shell = String::new();
    assert!(
        read_until(
            &mut pty,
            &mut shell,
            "handdrive-marker",
            std::time::Duration::from_secs(20)
        ),
        "the inline shell output renders: {shell:?}"
    );

    pty.write(b"/grants\r").expect("write /grants");
    let mut grants = String::new();
    assert!(
        read_until(
            &mut pty,
            &mut grants,
            "grants",
            std::time::Duration::from_secs(15)
        ),
        "the grants view renders: {grants:?}"
    );
    pty.write(b"\x1b").expect("close the overlay");
    std::thread::sleep(std::time::Duration::from_millis(500));
    pty.write(b"/exit\r").expect("write /exit");
    let log = find_session_log(&sandbox.state_dir()).expect("a session log");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        // Keep draining the pseudo-console (see the render test's note).
        let _ = read_until(
            &mut pty,
            &mut screen,
            "\u{0}",
            std::time::Duration::from_millis(50),
        );
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains("\"t\":\"session-end\"") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "a clean quit writes session-end"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// Verifies: the §14 keyboard row on Windows - Ctrl+C twice on an idle prompt
// exits and writes session-end (the first arms, the second leaves).
#[cfg(windows)]
#[test]
fn the_double_ctrl_c_exits_on_windows() {
    let _serial = conpty_serial();
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("tui-conpty-ctrlc");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let mut envs = console_env(&sandbox, &mock, false);
    envs.push(("OPENAI_MODEL", "test-model".to_string()));
    let mut pty = spawn_console(&sandbox, &envs);
    let mut screen = String::new();
    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "session in",
            std::time::Duration::from_secs(30)
        ),
        "startup renders: {screen:?}"
    );

    pty.write(b"\x03").expect("ctrl+c");
    std::thread::sleep(std::time::Duration::from_millis(400));
    pty.write(b"\x03").expect("ctrl+c again");
    // gh #122: a quit without messages writes no session log, so the
    // receipt is the exiting process (EOF on the drain) plus no
    // session directory left behind.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut exited = false;
    while std::time::Instant::now() < deadline {
        // Keep draining the pseudo-console (see the render test's
        // note); EOF (`None`) is the exited process.
        match pty.read(65536) {
            Ok(None) | Err(_) => {
                exited = true;
                break;
            }
            Ok(Some(_)) => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(exited, "double Ctrl+C exits");
    assert!(
        find_session_log(&sandbox.state_dir()).is_none(),
        "launch-and-quit writes no session log"
    );
}

// Verifies: G4 (issue #9) - the footer's first line is the working
// directory, `~`-shortened under home, with the git branch beside it -
// pi's `~/path (branch)` shape. The theme roles are untouched (the path
// wears the accent role, the rest the footer role).
#[cfg(unix)]
#[test]
fn the_footer_line_carries_the_working_directory_and_branch() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("footer-cwd");
    sandbox.approve_loopback_net(serde_json::json!({}));
    // A branch the footer can name: `.git/HEAD` is all `git_branch` reads.
    std::fs::create_dir_all(sandbox.project().join(".git")).expect("mkdir .git");
    std::fs::write(
        sandbox.project().join(".git").join("HEAD"),
        "ref: refs/heads/main\n",
    )
    .expect("write HEAD");

    let session = Tmux::new("footer-cwd");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    let pane = session.capture();
    let project = sandbox.project().display().to_string();
    assert!(
        pane.contains(&project),
        "the footer shows the working directory ({project}):\n{pane}"
    );
    assert!(
        pane.contains("(main)"),
        "the footer shows the git branch beside it:\n{pane}"
    );

    session.send(&["/exit", "Enter"]);
    // No turns ran, so no log exists for a session-end marker (gh
    // #122): the dead pane is the receipt.
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}

// Verifies: FR-CONC-1's turn boundary in the real interface - a cancelled
// turn must not poison the next one. The native provider's cancel flag
// used to latch (its `turn_started` was the trait's default no-op while
// `interrupt` flagged the capability engine): Ctrl+C once, and every
// later turn answered "request cancelled by the user (retries exhausted
// after 3)" until restart. The mock's first reply is delayed so the
// cancel lands while the call is genuinely in flight.
#[cfg(unix)]
#[test]
fn a_cancelled_turn_does_not_poison_the_next_one() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::SseAfter(8_000, sse_text("first reply")),
        Reply::Sse(sse_text("second reply")),
    ]));
    let sandbox = sandbox("cancel-next-turn");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("cancel-next");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    // Turn one, cancelled while its provider call is in flight.
    session.send(&["count to ten", "Enter"]);
    session.wait_for("running...", std::time::Duration::from_secs(10));
    session.send(&["C-c"]);
    session.wait_for("cancelled", std::time::Duration::from_secs(10));

    // Turn two runs normally: the leftover flag was cleared at the
    // boundary instead of pre-cancelling this call too.
    session.send(&["second turn", "Enter"]);
    session.wait_for("second reply", std::time::Duration::from_secs(25));

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: #109 (`lca hello` opens the TUI and submits "hello") — the
// live receipt: the positional arrives submitted on open and its turn
// runs to the mocked reply with no keypress typed.
#[cfg(any(unix, windows))]
#[cfg(unix)]
#[test]
fn gh109_positional_message_submits_on_open() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("turn done"))]));
    let sandbox = sandbox("tui-initial");
    sandbox.approve_loopback_net(serde_json::json!({}));

    // One unwrappable word: the pane wraps long user bands across rows,
    // so a multi-word needle never matches contiguously.
    let session = Tmux::new("initial");
    session.spawn(&sandbox, Some(&mock), true, &[], &["zebracake"]);
    let pane = session.wait_for("turn done", std::time::Duration::from_secs(25));
    assert!(
        pane.contains("zebracake"),
        "the positional arrived submitted on open, no keypress typed"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: #121 (the `$EDITOR` round trip through a real terminal: the
// composer text goes out to the editor process and comes back edited).
// Spaced paths ride the unit rows (they run on Windows CI too); this row
// proves the env-var path and the keybinding end to end.
#[cfg(any(unix, windows))]
#[cfg(unix)]
#[test]
fn gh121_external_editor_round_trips_in_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let sandbox = sandbox("tui-editor");
    let script = sandbox.root.join("mock-editor.py");
    std::fs::write(
        &script,
        "import sys\npath = sys.argv[1]\ntext = open(path, encoding=\"utf-8\").read()\nopen(path, \"w\", encoding=\"utf-8\").write(text + \"[edited]\")\n",
    )
    .expect("write mock editor");
    // Quoted inside the value: the spawn line interpolates env raw.
    let editor = format!("\"python3 {}\"", script.display());

    let session = Tmux::new("editor");
    session.spawn(&sandbox, None, false, &[("EDITOR", &editor)], &[]);
    session.wait_for("no model", std::time::Duration::from_secs(20));
    session.send(&["hello", "C-x", "C-e"]);
    session.wait_for("[edited]", std::time::Duration::from_secs(25));

    // The composer holds the edited text; submitting clears it (no model
    // here, so the turn refuses and the composer frees up for commands).
    session.send(&["Enter"]);
    session.wait_for("No model is active", std::time::Duration::from_secs(10));
    session.send(&["/exit", "Enter"]);
    // No turns ran, so no log exists for a session-end marker (gh
    // #122): the dead pane is the receipt.
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
