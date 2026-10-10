//! The cross-provider `/model` journeys (gh #177): the unified
//! catalog lists every ready provider's models, a pick from another
//! provider swaps the whole generation, and the turn that follows
//! routes to the new provider.
//!
//! Real terminal rows (tmux) against the local mock. Codex stands in
//! for the second provider: seeded credentials (an unexpired token,
//! an account, and an endpoint override at the mock) make it ready
//! without touching the network, and the mock answers its Responses
//! endpoint. Grok is left unseeded, so it stays unready and
//! contributes nothing - silently.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// Responses-shaped SSE saying one line (what the codex stream maps).
#[cfg(unix)]
fn responses_text(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    write!(
        out,
        "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{text}\"}}\n\n"
    )
    .expect("format");
    out.push_str(
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":3}}}\n\n",
    );
    out
}

/// Seed codex ready: an unexpired token (no refresh, no network), an
/// account, and the endpoint override pointing at the mock.
#[cfg(unix)]
fn seed_codex_ready(sandbox: &Sandbox, mock: &Mock) {
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
        + 3600;
    sandbox.write_credentials(
        "codex",
        serde_json::json!({
            "access": "seeded-token",
            "expires": expires.to_string(),
            "account_id": "seeded-account",
            "api_base": mock.url(),
        }),
    );
}

// Verifies: gh #177 acceptances 1-3 - `/model` lists the ready second
// provider's models tagged with it, selecting one swaps provider and
// model mid-session (footer, notice, `model-change` carrying both,
// `meta.json`), and the next turn routes to the new provider. The way
// out (a typed `provider/model` pick back) rides the same routine.
#[cfg(unix)]
#[test]
fn selecting_another_providers_model_switches_and_routes() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(responses_text("switched turn")),
        Reply::Sse(sse_text("back on openai")),
    ]));
    let sandbox = sandbox("gh177-switch");
    sandbox.approve_loopback_net(serde_json::json!({}));
    // The second provider arrives as an installed wasm extension (only
    // openai-compatible ships bundled); the fixture component is
    // rebuilt at publish time like every fixture component.
    sandbox.install_component("codex", CODEX_MANIFEST, CODEX_COMPONENT);
    seed_codex_ready(&sandbox, &mock);

    let session = Tmux::new("gh177-switch");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    // The catalog names the ready second provider's model with it.
    session.send(&["/model", "Enter"]);
    session.wait_for("gpt-5.4-mini (codex)", std::time::Duration::from_secs(10));
    // Filter to its row and apply: the pick swaps the generation, and
    // the notice and footer agree on where it landed.
    session.send(&["codex", "Enter"]);
    session.wait_for(
        "model for this session: codex/gpt-5.4-mini",
        std::time::Duration::from_secs(10),
    );
    session.wait_for("codex/gpt-5.4-mini", std::time::Duration::from_secs(10));

    // The turn that follows routes to the new provider.
    session.send(&["hello", "Enter"]);
    session.wait_for("switched turn", std::time::Duration::from_secs(25));
    let bodies = mock.bodies().join("\n");
    assert!(
        bodies.contains("\"model\":\"gpt-5.4-mini\""),
        "the switched-to model is the one on the wire: {bodies}"
    );

    // The way out is a typed cross-provider pick back.
    session.send(&["/model test-model", "Enter"]);
    session.wait_for(
        "model for this session: openai-compatible/test-model",
        std::time::Duration::from_secs(10),
    );
    session.send(&["hello again", "Enter"]);
    session.wait_for("back on openai", std::time::Duration::from_secs(25));

    session.send(&["/exit", "Enter"]);
    let log = wait_for_session_end(&sandbox.state_dir(), std::time::Duration::from_secs(15));
    let changes: Vec<(Option<String>, Option<String>, String)> = log
        .lines()
        .filter(|line| line.contains(r#""t":"model-change""#))
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .map(|change| {
            (
                change
                    .get("from")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                change
                    .get("to")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                change
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        changes,
        vec![
            (
                Some("test-model".to_string()),
                Some("gpt-5.4-mini".to_string()),
                "codex".to_string()
            ),
            (
                Some("gpt-5.4-mini".to_string()),
                Some("test-model".to_string()),
                "openai-compatible".to_string()
            ),
        ],
        "one record per switch, each carrying both sides"
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
        Some("test-model"),
        "gh #20's write landed where the walk stopped: {meta}"
    );
}

// Verifies: gh #177 acceptance 3's headless half - a turn addressed
// to the second provider routes through the kit-migrated Responses
// engine with no interface involved (what a session runs after a
// switch, minus the switching).
#[cfg(unix)]
#[test]
fn a_headless_turn_to_the_second_provider_routes() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(responses_text(
        "headless codex",
    ))]));
    let sandbox = sandbox("gh177-headless");
    sandbox.install_component("codex", CODEX_MANIFEST, CODEX_COMPONENT);
    seed_codex_ready(&sandbox, &mock);
    // Headless `--provider` is a profile scope, not a provider switch
    // (gh #31): the session's provider comes from the config file.
    std::fs::create_dir_all(sandbox.state_dir()).expect("mkdir .lca");
    std::fs::write(
        sandbox.state_dir().join("config.toml"),
        "provider = \"codex\"\n",
    )
    .expect("write config");

    let output = sandbox.run(
        Some(&mock),
        &[
            "--model",
            "gpt-5.4-mini",
            "--allow-host",
            "127.0.0.1",
            "-p",
            "hi",
        ],
    );
    let out = String::from_utf8_lossy(&output.stdout).into_owned();
    let err = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(
        output.status.code(),
        Some(0),
        "the headless turn succeeds: stdout={out} stderr={err}"
    );
    assert!(
        out.contains("headless codex"),
        "the second provider answered: {out}"
    );
    let bodies = mock.bodies().join("\n");
    assert!(
        bodies.contains("\"model\":\"gpt-5.4-mini\""),
        "the addressed model is the one on the wire: {bodies}"
    );
}

// Verifies: gh #177 acceptance 1's silent half - an unready provider
// (grok, nothing stored) contributes no models: naming one refuses
// with the real alternatives instead of reaching it.
#[cfg(unix)]
#[test]
fn an_unready_provider_contributes_nothing_silently() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("gh177-unready");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("gh177-unready");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for(
        "openai-compatible/test-model",
        std::time::Duration::from_secs(20),
    );

    session.send(&["/model grok-4.20", "Enter"]);
    let pane = session.wait_for("no model named", std::time::Duration::from_secs(10));
    assert!(
        pane.contains("grok-4.20"),
        "the refusal names what was asked: {pane}"
    );
    assert!(
        !pane.contains("grok-4.20 (grok)"),
        "the unready provider offers no rows: {pane}"
    );

    session.send(&["/exit", "Enter"]);
    // No turns ran, so no log exists for a session-end marker (gh
    // #122): the dead pane is the receipt.
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}

// Verifies: gh #211's picker half - a keyless `auth = "none"` endpoint
// lists with no key anywhere: the catalog probe reports ready, so the
// configured model reaches the picker (discovery has nothing to say to
// an unreachable endpoint, and no login ever ran to discover with).
#[cfg(unix)]
#[test]
fn a_keyless_local_endpoint_lists_its_models() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let sandbox = sandbox("gh211-ollama");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("gh211-ollama");
    session.spawn(
        &sandbox,
        None,
        false,
        &[
            ("OPENAI_BASE_URL", "http://localhost:11434/v1"),
            ("OPENAI_MODEL", "qwen2.5-coder:7b"),
        ],
        &[],
    );
    // Keyless starts model-less (startup still uses the credentials
    // heuristic for the initial pick); the catalog probe is what
    // reports ready, so the picker is where the row appears.
    session.wait_for("[session in", std::time::Duration::from_secs(20));

    session.send(&["/model", "Enter"]);
    let pane = session.wait_for("qwen2.5-coder:7b", std::time::Duration::from_secs(10));
    assert!(
        pane.contains("qwen2.5-coder:7b (localhost)"),
        "the row names the service that would bill the call: {pane}"
    );

    session.send(&["Escape"]);
    std::thread::sleep(std::time::Duration::from_millis(400));
    session.send(&["/exit", "Enter"]);
    // No turns ran, so no log exists for a session-end marker (gh
    // #122): the dead pane is the receipt.
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
