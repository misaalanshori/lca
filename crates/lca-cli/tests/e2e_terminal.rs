//! End-to-end terminal tests: the real-terminal tmux paths and the login pickers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;
use std::process::{Command, Output};

#[cfg(unix)]
struct Tmux {
    name: String,
}

#[cfg(unix)]
impl Tmux {
    fn new(tag: &str) -> Tmux {
        let name = format!("lca-smoke-{}-{tag}", std::process::id());
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &name])
            .status();
        Tmux { name }
    }

    fn tmux(args: &[&str]) -> Output {
        Command::new("tmux").args(args).output().expect("run tmux")
    }

    fn capture(&self) -> String {
        let out = Self::tmux(&["capture-pane", "-t", &self.name, "-p"]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn spawn(
        &self,
        sandbox: &Sandbox,
        mock: Option<&Mock>,
        with_key: bool,
        extra_env: &[(&str, &str)],
        args: &[&str],
    ) {
        let key = if with_key {
            " OPENAI_API_KEY=test-key"
        } else {
            ""
        };
        let endpoint = mock
            .map(|mock| format!(" OPENAI_BASE_URL={}", mock.url()))
            .unwrap_or_default();
        // Issue #3: no implicit default model, so a mock-backed session picks
        // one explicitly. `extra_env` (appended last) can still override it.
        let model = if mock.is_some() {
            " OPENAI_MODEL=test-model"
        } else {
            ""
        };
        let extra: String = extra_env
            .iter()
            .map(|(key, value)| format!(" {key}={value}"))
            .collect();
        let arguments: String = args.iter().map(|arg| format!(" {arg}")).collect();
        let command = format!(
            "cd {project} && HOME={home} USERPROFILE={home} XDG_DATA_HOME={data} \
             APPDATA={data} LOCALAPPDATA={data} XDG_CONFIG_HOME={config} \
             LCA_UPDATE_CHECK=false{endpoint}{key}{model}{extra} {bin}{arguments}",
            project = sandbox.project().display(),
            home = sandbox.home.display(),
            data = sandbox.data.display(),
            config = sandbox.home.join(".config").display(),
            bin = env!("CARGO_BIN_EXE_lca"),
        );
        let out = Self::tmux(&[
            "new-session",
            "-d",
            "-s",
            &self.name,
            "-x",
            "140",
            "-y",
            "40",
            &command,
        ]);
        assert!(
            out.status.success(),
            "tmux new-session: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn send(&self, keys: &[&str]) {
        let mut args = vec!["send-keys", "-t", &self.name];
        args.extend_from_slice(keys);
        let out = Self::tmux(&args);
        assert!(
            out.status.success(),
            "tmux send-keys: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn wait_for(&self, needle: &str, timeout: std::time::Duration) -> String {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let pane = self.capture();
            if pane.contains(needle) {
                return pane;
            }
            if std::time::Instant::now() > deadline {
                panic!("`{needle}` never appeared in the pane:\n{pane}");
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }

    fn resize(&self, cols: u32, rows: u32) -> String {
        let out = Self::tmux(&[
            "resize-window",
            "-t",
            &self.name,
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
        ]);
        assert!(
            out.status.success(),
            "tmux resize-window: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        std::thread::sleep(std::time::Duration::from_millis(500));
        self.capture()
    }
}

#[cfg(unix)]
impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &self.name])
            .status();
    }
}

#[cfg(any(unix, windows))]
#[cfg(unix)]
#[test]
fn the_tui_renders_a_turn_in_a_real_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("shell", r#"{"command":"echo smoke-ok"}"#)),
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

// Verifies: ADR-0033 / `api-key-login-plan.md` D1 - `/login` with no
// argument is a list picker: the extension's presets, and always the host's
// universal "Custom endpoint..." entry. The list is longer than the box, so
// it scrolls - and the universal entry, which sits last, has to stay
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
        "the universal entry is last, below the fold at the top:\n{pane}"
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
    assert!(reached, "the host's universal entry is reachable");
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
    // hint names a local host rather than counting rows.
    let mut reached = false;
    for _ in 0..24 {
        session.send(&["Down"]);
        std::thread::sleep(std::time::Duration::from_millis(120));
        if session.capture().contains("localhost") {
            reached = true;
            break;
        }
    }
    assert!(reached, "the walk reached a local preset");
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
    let budget = [
        ("OPENAI_CONTEXT_WINDOW", "1000"),
        ("LCA_COMPACTION_THRESHOLD", "0.01"),
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
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

// Verifies: the real-terminal checklist (docs/testing-plan.md section 14) on
// the Windows harness: startup renders, a scripted turn streams and renders,
// and a clean quit writes `session-end`.
#[cfg(windows)]
#[ignore = "ConPTY produced no bytes on the hosted runner (docs/platform-notes.md, Windows quarantine ledger)"]
#[test]
fn the_tui_renders_a_turn_in_a_windows_console() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text_with_usage(
        "console smoke ok",
        20,
        0,
    ))]));
    let sandbox = sandbox("tui-conpty");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let envs = console_env(&sandbox, &mock, true);
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

    pty.write(b"hello console\r").expect("write the prompt");
    assert!(
        read_until(
            &mut pty,
            &mut screen,
            "console smoke ok",
            std::time::Duration::from_secs(60)
        ),
        "the reply renders: {screen:?}"
    );

    pty.write(b"/exit\r").expect("write /exit");
    let log = find_session_log(&sandbox.state_dir()).expect("a session log");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
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

// Verifies: the real-terminal checklist's secret-prompt case on Windows -
// what the user types into `/login` never reaches the visible screen.
#[cfg(windows)]
#[ignore = "ConPTY produced no bytes on the hosted runner (docs/platform-notes.md, Windows quarantine ledger)"]
#[test]
fn the_logins_secret_prompt_masks_input_in_a_windows_console() {
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
    pty.write(b"/login\r").expect("write /login");
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

    // `/login` opens its picker: zero providers means no presets, but the
    // host's universal entry is always there and is the way back.
    session.send(&["/login", "Enter"]);
    session.wait_for("Custom endpoint", std::time::Duration::from_secs(15));
    let pane = session.capture();
    assert!(
        pane.contains("Custom endpoint"),
        "the picker still offers a way to configure one: {pane}"
    );

    // Walk to it and complete the three fields: the state has to be
    // *leavable*, not merely reportable.
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
    // durable state it left behind rather than a flash of text.
    std::thread::sleep(std::time::Duration::from_millis(800));
    // And the project's enablement actually moved.
    let grants: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(sandbox.state_dir().join("grants.json")).expect("grants"),
    )
    .expect("grants json");
    assert_eq!(
        grants["projects"][sandbox.project().to_string_lossy().to_string()]["extensions"]["openai-compatible"],
        serde_json::json!(true),
        "the provider is enabled again"
    );
}
