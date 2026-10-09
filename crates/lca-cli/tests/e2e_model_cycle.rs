//! The model-workflow journeys (gh #8 / EFG-003): the cycle keys, the
//! enabled-model scope, and the `model-change` record each step appends.
//!
//! Real terminal rows (tmux) against the local mock: the keys must reach
//! the interface the way a person presses them, and the log must say what
//! the pane showed. `alt+p` is the backward key here because tmux cannot
//! report `Shift+Ctrl+P`; it is the same action (pi keeps it for exactly
//! this reason) and the registry pins both keys.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// Two models in scope: the env one and the one the credentials add, in
/// the provider's own order (the picker showed them in this order before).
/// Gated like the import above: `Sandbox` only exists where `common::*`
/// does, and the rows that call this are Unix-only anyway (cross-target
/// clippy, Windows).
#[cfg(unix)]
fn two_models(sandbox: &Sandbox) {
    sandbox.write_credentials(
        "openai-compatible",
        serde_json::json!({ "models": "second-model" }),
    );
}

/// Every `model-change` record in a log's text, as `(from, to)` pairs.
/// Only the Unix rows read logs, so it is gated with them.
#[cfg(unix)]
fn model_changes(log: &str) -> Vec<(Option<String>, String)> {
    log.lines()
        .filter(|line| line.contains(r#""t":"model-change""#))
        .filter_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            Some((
                value
                    .get("from")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                value.get("to").and_then(|v| v.as_str())?.to_string(),
            ))
        })
        .collect()
}

// Verifies: gh #8 acceptance (1) - Ctrl+P forward and the backward key
// are one motion each: forward leaves the original, backward returns to
// it, and the turn that follows runs on whichever model the pane showed
// (the cycle is a `/model` switch, not a display change).
#[cfg(unix)]
#[test]
fn ctrl_p_then_the_backward_key_returns_to_the_original_model() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("cycled turn"))]));
    let sandbox = sandbox("gh8-cycle-return");
    sandbox.approve_loopback_net(serde_json::json!({}));
    two_models(&sandbox);

    let session = Tmux::new("gh8-cycle-return");
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

    // Forward: the footer moves to the second model.
    session.send(&["C-p"]);
    session.wait_for(
        "openai-compatible/second-model",
        std::time::Duration::from_secs(10),
    );

    // Backward: back to the original, one keypress.
    session.send(&["M-p"]);
    session.wait_for(
        "openai-compatible/first-model",
        std::time::Duration::from_secs(10),
    );

    // And the model the pane shows is the model the next request uses.
    session.send(&["hello", "Enter"]);
    session.wait_for("cycled turn", std::time::Duration::from_secs(25));
    let bodies = mock.bodies().join("\n");
    assert!(
        bodies.contains("\"model\":\"first-model\""),
        "the cycled-to model is the one on the wire: {bodies}"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: gh #8 acceptance (1) - the cycle wraps at the end of the
// scope, every step appends one `model-change` record naming the model it
// left and the model it took, and `meta.model` lands on where the walk
// stopped (gh #20's write riding along).
#[cfg(unix)]
#[test]
fn cycling_wraps_and_every_step_appends_a_model_change_record() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("gh8-cycle-records");
    sandbox.approve_loopback_net(serde_json::json!({}));
    two_models(&sandbox);

    let session = Tmux::new("gh8-cycle-records");
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

    // four steps over two models: out, back, out, wrap.
    for key in ["C-p", "M-p", "C-p", "C-p"] {
        session.send(&[key]);
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    // second, first, second, first (wrap) - the pane says where it ended.
    let pane = session.wait_for(
        "openai-compatible/first-model",
        std::time::Duration::from_secs(10),
    );
    assert!(
        pane.contains("first-model"),
        "the wrap landed on the first model:\n{pane}"
    );
    session.send(&["/exit", "Enter"]);
    let log = wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));

    let changes = model_changes(&log);
    assert_eq!(
        changes,
        vec![
            (Some("first-model".to_string()), "second-model".to_string()),
            (Some("second-model".to_string()), "first-model".to_string()),
            (Some("first-model".to_string()), "second-model".to_string()),
            (Some("second-model".to_string()), "first-model".to_string()),
        ],
        "one record per step, each naming what it left and what it took"
    );

    let log_path = find_session_log(&sandbox.state_dir()).expect("a session log");
    let meta = std::fs::read_to_string(
        log_path
            .parent()
            .expect("session directory")
            .join("meta.json"),
    )
    .expect("meta.json");
    let value: serde_json::Value = serde_json::from_str(&meta).expect("meta.json parses");
    assert_eq!(
        value.get("model").and_then(serde_json::Value::as_str),
        Some("first-model"),
        "gh #20's write landed where the walk stopped: {meta}"
    );
}

// Verifies: gh #8 acceptance (scope) - `models.enabled` in the config
// file cuts *both* halves: the picker lists only the scoped model, the
// cycle has nothing to cycle to (pi's "only one model in scope"), and the
// session starts on the scoped model rather than the provider's first.
#[cfg(unix)]
#[test]
fn the_configured_scope_cuts_the_cycle_and_the_picker() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("gh8-scope-config");
    sandbox.approve_loopback_net(serde_json::json!({}));
    two_models(&sandbox);
    std::fs::create_dir_all(sandbox.state_dir()).expect("mkdir .lca");
    std::fs::write(
        sandbox.state_dir().join("config.toml"),
        "models.enabled = [\"second-model\"]\n",
    )
    .expect("write config");

    let session = Tmux::new("gh8-scope-config");
    session.spawn(
        &sandbox,
        Some(&mock),
        true,
        &[("OPENAI_MODEL", "first-model")],
        &[],
    );
    // The scope picks the startup model too: only second-model exists here.
    session.wait_for(
        "openai-compatible/second-model",
        std::time::Duration::from_secs(20),
    );

    // The picker's listing is the scope.
    session.send(&["/model", "Enter"]);
    let pane = session.wait_for("second-model (", std::time::Duration::from_secs(15));
    assert!(
        !pane.contains("first-model ("),
        "the scope cuts the listing:\n{pane}"
    );
    session.send(&["Escape"]);
    std::thread::sleep(std::time::Duration::from_millis(400));

    // And the cycle has nowhere to go.
    session.send(&["C-p"]);
    let pane = session.wait_for(
        "only one model in scope",
        std::time::Duration::from_secs(10),
    );
    assert!(
        pane.contains("only one model in scope"),
        "the singleton message names the scope, not the provider:\n{pane}"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: gh #8 acceptance (scope) - `--models a,b` is the same scope
// through the flag layer: it cuts the listing, it decides which model the
// session starts on (flags beat the environment, FR-CFG-1), and the cycle
// reports the scoped singleton.
#[cfg(unix)]
#[test]
fn the_models_flag_sets_the_scope_for_the_run() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("gh8-scope-flag");
    sandbox.approve_loopback_net(serde_json::json!({}));
    two_models(&sandbox);

    let session = Tmux::new("gh8-scope-flag");
    session.spawn(
        &sandbox,
        Some(&mock),
        true,
        &[("OPENAI_MODEL", "first-model")],
        &["--models", "second-model"],
    );
    session.wait_for(
        "openai-compatible/second-model",
        std::time::Duration::from_secs(20),
    );

    session.send(&["/model", "Enter"]);
    let pane = session.wait_for("second-model (", std::time::Duration::from_secs(15));
    assert!(
        !pane.contains("first-model ("),
        "`--models` cuts the listing the same way:\n{pane}"
    );
    session.send(&["Escape"]);
    std::thread::sleep(std::time::Duration::from_millis(400));

    session.send(&["C-p"]);
    session.wait_for(
        "only one model in scope",
        std::time::Duration::from_secs(10),
    );

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: gh #8 acceptance (2) - Ctrl+S in the picker persists the
// highlighted model through the comment-preserving writer `/thinking`
// and `/theme` use (the file's comments and its other keys survive), and
// a fresh process with no `--model` and no `OPENAI_MODEL` resolves that
// saved default - the two halves of "save default" in one journey.
#[cfg(unix)]
#[test]
fn ctrl_s_saves_the_default_and_a_fresh_session_resolves_it() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("gh8-save-default");
    sandbox.approve_loopback_net(serde_json::json!({}));
    two_models(&sandbox);
    std::fs::create_dir_all(sandbox.state_dir()).expect("mkdir .lca");
    // A config with comments and a key Ctrl+S does not own.
    std::fs::write(
        sandbox.state_dir().join("config.toml"),
        "# my setup\n\nprovider = \"openai-compatible\"\n# keep me\ncompaction.threshold = 0.7\n",
    )
    .expect("write config");

    let session = Tmux::new("gh8-save-default");
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

    // Pick the second row, save it with Ctrl+S.
    session.send(&["/model", "Enter"]);
    session.wait_for("second-model (", std::time::Duration::from_secs(15));
    session.send(&["Down"]);
    std::thread::sleep(std::time::Duration::from_millis(300));
    session.send(&["C-s"]);
    session.wait_for(
        "default model saved: second-model",
        std::time::Duration::from_secs(10),
    );
    session.send(&["Escape"]);
    std::thread::sleep(std::time::Duration::from_millis(400));

    // The writer kept the file intact around the one key it wrote.
    let text = std::fs::read_to_string(sandbox.state_dir().join("config.toml")).expect("config");
    assert!(
        text.contains("# my setup"),
        "comments survive Ctrl+S:\n{text}"
    );
    assert!(
        text.contains("# keep me"),
        "the second comment too:\n{text}"
    );
    assert!(
        text.contains("compaction.threshold = 0.7"),
        "the other key survives:\n{text}"
    );
    assert!(
        text.contains("model = \"second-model\""),
        "the default is persisted:\n{text}"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));

    // A fresh process, no `--model`, no `OPENAI_MODEL`: the file's value
    // is the one that resolves.
    let again = Tmux::new("gh8-save-default-fresh");
    again.spawn(&sandbox, Some(&mock), true, &[], &[]);
    again.wait_for(
        "openai-compatible/second-model",
        std::time::Duration::from_secs(20),
    );
    again.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: gh #8 / EFG-041 - `--model <pattern>` fuzzy-resolves against
// the provider's list (a prefix is a pattern, not a miss) and the
// `:thinking` suffix becomes this session's level: the footer says both.
#[cfg(unix)]
#[test]
fn the_model_flag_fuzzy_resolves_and_its_suffix_sets_the_level() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("gh8-flag-pattern");
    sandbox.approve_loopback_net(serde_json::json!({}));
    two_models(&sandbox);

    let session = Tmux::new("gh8-flag-pattern");
    session.spawn(
        &sandbox,
        Some(&mock),
        true,
        &[("OPENAI_MODEL", "first-model")],
        &["--model", "second:high"],
    );
    let pane = session.wait_for(
        "openai-compatible/second-model",
        std::time::Duration::from_secs(20),
    );
    let footer = pane
        .lines()
        .find(|line| line.contains("ctx"))
        .unwrap_or(&pane);
    assert!(
        !footer.contains("first-model"),
        "the pattern resolved to the model it named:\n{footer}"
    );
    assert!(
        footer.contains("high"),
        "the `:high` suffix is the session's level:\n{footer}"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: gh #8 acceptance (3) - `--thinking <level>` is the session
// default: the footer wears it from the first frame, and the flag
// accepts pi's vocabulary.
#[cfg(unix)]
#[test]
fn the_thinking_flag_is_the_session_default() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("gh8-thinking-flag");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("gh8-thinking-flag");
    session.spawn(
        &sandbox,
        Some(&mock),
        true,
        &[("OPENAI_MODEL", "test-model")],
        &["--thinking", "low"],
    );
    let pane = session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );
    let footer = pane
        .lines()
        .find(|line| line.contains("ctx"))
        .unwrap_or(&pane);
    assert!(
        footer.contains("low"),
        "the flag's level is on the footer from the first frame:\n{footer}"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: gh #8 phase 4 (pi's `modelThinkingLevels`) - a level the
// model does not offer is clamped into its set: `--model second:high`
// starts on the model's own default instead of `high`, and `/thinking`
// saying so rather than storing a level the model refuses.
#[cfg(unix)]
#[test]
fn a_thinking_suffix_outside_the_models_set_is_clamped_to_it() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("gh8-thinking-clamp");
    sandbox.approve_loopback_net(serde_json::json!({}));
    two_models(&sandbox);
    std::fs::create_dir_all(sandbox.state_dir()).expect("mkdir .lca");
    std::fs::write(
        sandbox.state_dir().join("config.toml"),
        "[models.thinking_levels]\n\"second-model\" = [\"low\"]\n",
    )
    .expect("write config");

    let session = Tmux::new("gh8-thinking-clamp");
    session.spawn(
        &sandbox,
        Some(&mock),
        true,
        &[("OPENAI_MODEL", "first-model")],
        &["--model", "second:high"],
    );
    let pane = session.wait_for(
        "openai-compatible/second-model",
        std::time::Duration::from_secs(20),
    );
    let footer = pane
        .lines()
        .find(|line| line.contains("ctx"))
        .unwrap_or(&pane);
    assert!(footer.contains("second-model"), "{footer}");
    assert!(
        footer.contains("low") && !footer.contains("high"),
        "`:high` is clamped into this model's set (low is its default):\n{footer}"
    );

    // The picker hides what the model refuses (gh #41): `high` is
    // not offered at all, so one row down from unset is `low` and the
    // notice names what landed with no clamp detour.
    session.send(&["/thinking", "Enter"]);
    let text = session.wait_for(
        "unset (provider default)",
        std::time::Duration::from_secs(10),
    );
    assert!(text.contains("low"), "the offered level shows: {text}");
    assert!(
        !text.contains("high"),
        "the refused level is hidden, not offered: {text}"
    );
    session.send(&["Down"]);
    session.send(&["Enter"]);
    session.wait_for("thinking: low", std::time::Duration::from_secs(10));

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}

// Verifies: gh #8 phase 4 - a switch applies the new model's configured
// default (the first allowed level, pi's per-model precedence), and a
// `:thinking` suffix the model *does* offer passes through untouched -
// in the flag and in `/model` alike.
#[cfg(unix)]
#[test]
fn a_switch_applies_the_models_thinking_default_and_an_allowed_suffix_lands() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("gh8-thinking-default");
    sandbox.approve_loopback_net(serde_json::json!({}));
    two_models(&sandbox);
    std::fs::create_dir_all(sandbox.state_dir()).expect("mkdir .lca");
    std::fs::write(
        sandbox.state_dir().join("config.toml"),
        "[models.thinking_levels]\n\"second-model\" = [\"medium\", \"high\"]\n",
    )
    .expect("write config");

    let session = Tmux::new("gh8-thinking-default");
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

    // The cycle to a model with a configured default applies it.
    session.send(&["C-p"]);
    let pane = session.wait_for(
        "openai-compatible/second-model",
        std::time::Duration::from_secs(10),
    );
    let footer = pane
        .lines()
        .find(|line| line.contains("ctx"))
        .unwrap_or(&pane);
    assert!(footer.contains("second-model"), "{footer}");
    assert!(
        footer.contains("medium"),
        "the switch applied the model's configured default:\n{footer}"
    );

    // A suffix the model offers lands as asked, through `/model` too.
    for c in "/model second:high".chars() {
        session.send(&[&c.to_string()]);
    }
    session.send(&["Enter"]);
    // Outcome-based (the footer), never the notice: a "model for this
    // session" line from the earlier switch can still be on screen and
    // satisfy a notice wait before `:high` lands (CI flake, twice).
    session.wait_for(
        "/second-model \u{2022} high",
        std::time::Duration::from_secs(10),
    );
    let pane = session.capture();
    let footer = pane
        .lines()
        .find(|line| line.contains("ctx"))
        .unwrap_or(&pane);
    assert!(footer.contains("second-model"), "{footer}");
    assert!(
        footer.contains("high"),
        "`:high` is in this model's set, so it lands:\n{footer}"
    );

    session.send(&["/exit", "Enter"]);
    wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
}
