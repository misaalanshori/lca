//! Model-resolution rows (UI/UX Phase 4, register G1 and G2): the startup
//! path a user actually hits - env-key provider, trusted grant, no config
//! file, with and without `--model` - and the `/model` picker's label
//! safety half. Split from `e2e_terminal.rs`, which sits near the
//! workspace's 1,200-line ceiling (Gate 11).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

// The rows are Unix-only (tmux), so the glob import is too.
#[cfg(unix)]
use common::*;

// ---------------------------------------------------------------------------
// G1 (UI/UX Phase 4, register G1): the startup model-resolution rows the
// suite never had - the plain path an actual user hits. Fresh HOME, env-key
// provider, a trusted project grant, no config file. The coverage gap above
// is what made a phantom regression undecidable; these two rows decide it.
// ---------------------------------------------------------------------------

// Verifies: G1(a) - no flags: the footer resolves the env provider's model
// (`openai-compatible/<OPENAI_MODEL>`) and a turn runs against it.
#[cfg(unix)]
#[test]
fn startup_from_env_resolves_the_model_and_runs_a_turn() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text_with_usage(
        "env startup turn",
        40,
        0,
    ))]));
    let sandbox = sandbox("startup-env");
    // The trusted project grant (and the loopback consent the mock needs).
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("startup-env");
    session.spawn(
        &sandbox,
        Some(&mock),
        true,
        &[("OPENAI_MODEL", "deepseek-v4.1-flash")],
        &[],
    );

    // 1. The footer resolves the model from the environment alone.
    session.wait_for(
        "openai-compatible/deepseek-v4.1-flash",
        std::time::Duration::from_secs(20),
    );

    // 2. A turn runs against that model.
    session.send(&["hello", "Enter"]);
    session.wait_for("env startup turn", std::time::Duration::from_secs(25));

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: G1(b) - `--model` wins over the environment and over the config
// file at initial resolution: the footer shows the flag's id and the
// provider call carries it raw.
#[cfg(unix)]
#[test]
fn the_model_flag_beats_env_and_config_at_startup() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text_with_usage(
        "flag startup turn",
        40,
        0,
    ))]));
    let sandbox = sandbox("startup-flag");
    sandbox.approve_loopback_net(serde_json::json!({}));
    // Both lower-precedence sources name a model of their own (FR-CFG-1:
    // flags > environment > file), so only the flag can be showing.
    std::fs::create_dir_all(sandbox.state_dir()).expect("mkdir .lca");
    std::fs::write(
        sandbox.state_dir().join("config.toml"),
        "model = \"config-model\"\n",
    )
    .expect("write config");

    let session = Tmux::new("startup-flag");
    session.spawn(
        &sandbox,
        Some(&mock),
        true,
        &[("OPENAI_MODEL", "env-model")],
        &["--model", "flag-model"],
    );

    let pane = session.wait_for(
        "openai-compatible/flag-model",
        std::time::Duration::from_secs(20),
    );
    assert!(
        !pane.contains("env-model") && !pane.contains("config-model"),
        "the flag's id is the one on screen:\n{pane}"
    );

    session.send(&["hello", "Enter"]);
    session.wait_for("flag startup turn", std::time::Duration::from_secs(25));

    // The provider call carries the raw flag id, not a label and not the
    // env/config ids.
    let bodies = mock.bodies().join("\n");
    assert!(
        bodies.contains("\"model\":\"flag-model\""),
        "the provider call sends the flag's id: {bodies}"
    );
    assert!(
        !bodies.contains("env-model") && !bodies.contains("config-model"),
        "neither lower-precedence id reaches the wire: {bodies}"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: G2 (issue #3's safety half) end to end - the `/model` picker
// shows `model (provider)`, and the row that is picked reaches the
// provider call, the session metadata, and the log as the raw id. The
// decorated label is display text; it never becomes an identifier.
#[cfg(unix)]
#[test]
fn the_model_picker_label_never_reaches_the_provider_call() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal tests are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text_with_usage(
        "picked turn",
        40,
        0,
    ))]));
    let sandbox = sandbox("picker-label");
    sandbox.approve_loopback_net(serde_json::json!({}));
    // A second model beside the env one, so the pick has to move rows.
    sandbox.write_credentials(
        "openai-compatible",
        serde_json::json!({ "models": "second-model" }),
    );

    let session = Tmux::new("picker-label");
    session.spawn(
        &sandbox,
        Some(&mock),
        true,
        &[("OPENAI_MODEL", "first-model")],
        &[],
    );
    session.wait_for(
        "openai-compatible/first-model",
        std::time::Duration::from_secs(20),
    );

    // 1. The picker shows the decorated label (issue #3's display half).
    session.send(&["/model", "Enter"]);
    let pane = session.wait_for(
        "second-model (openai-compatible)",
        std::time::Duration::from_secs(15),
    );
    assert!(
        pane.contains("first-model (openai-compatible)"),
        "every row carries its provider label:\n{pane}"
    );

    // 2. Selecting the second row announces the switch with the raw id.
    session.send(&["Down"]);
    std::thread::sleep(std::time::Duration::from_millis(300));
    session.send(&["Enter"]);
    session.wait_for(
        "model for this session: openai-compatible/second-model",
        std::time::Duration::from_secs(15),
    );

    // 3. The turn's provider call carries the raw id.
    session.send(&["hello", "Enter"]);
    session.wait_for("picked turn", std::time::Duration::from_secs(25));
    let bodies = mock.bodies().join("\n");
    assert!(
        bodies.contains("\"model\":\"second-model\""),
        "the provider call sends the raw id: {bodies}"
    );
    assert!(
        !bodies.contains("second-model (openai-compatible)"),
        "the label never reaches the wire: {bodies}"
    );

    session.send(&["/exit", "Enter"]);
    let log = wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));

    // 4. Session metadata carries the raw id too: the assistant record
    //    and meta.json.
    assert!(
        log.contains("\"model\":\"second-model\""),
        "the assistant record stores the raw id"
    );
    assert!(
        !log.contains("second-model (openai-compatible)"),
        "the label never enters the session log"
    );
    // Session metadata's model lives on the session's records (meta.json
    // carries no model field today); either way the label never enters it.
    let meta = std::fs::read_to_string(
        find_session_log(&sandbox.state_dir())
            .expect("a session log")
            .parent()
            .expect("session directory")
            .join("meta.json"),
    )
    .expect("meta.json");
    assert!(
        !meta.contains("second-model (openai-compatible)"),
        "the label never enters session metadata: {meta}"
    );
}
