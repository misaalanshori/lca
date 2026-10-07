//! End-to-end tests: one headless turn, the JSON envelope, and the exit codes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;

#[test]
fn headless_prompt_writes_the_result_to_stdout() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text_with_usage(
        "Hello from mock",
        1200,
        1000,
    ))]));
    let box_ = sandbox("headless");
    let output = box_.run(Some(&mock), &["-p", "hi"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("Hello from mock"),
        "stdout: {}",
        stdout(&output)
    );
    assert_eq!(mock.request_count(), 1, "exactly one completion request");
}

// The --json envelope contract (docs/headless.md): one object per line,
// each with a type; the last line is turn-end.
#[test]
fn json_mode_emits_one_typed_object_per_line() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text_with_usage(
        "done", 1200, 1000,
    ))]));
    let box_ = sandbox("json");
    let output = box_.run(Some(&mock), &["-p", "hi", "--json"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));

    let lines = json_lines(&output);
    assert!(!lines.is_empty());
    // gh #44: the extended taxonomy. New kinds arrive only by design
    // (docs/headless.md); anything else is still a bug.
    let allowed = [
        "session-start",
        "turn-start",
        "message-start",
        "message-end",
        "text",
        "text-delta",
        "thinking-delta",
        "tool-call",
        "tool-update",
        "tool-result",
        "usage",
        "extension-event",
        "queue-queued",
        "queue-flushed",
        "compaction-start",
        "compaction-end",
        "retry-scheduled",
        "retry-end",
        "error",
        "turn-end",
        "response",
    ];
    for line in &lines {
        let kind = line["type"].as_str().expect("type field");
        assert!(
            allowed.contains(&kind),
            "unknown envelope type {kind}: {line}"
        );
    }
    let last = lines.last().expect("turn-end line");
    assert_eq!(last["type"], "turn-end");
    assert_eq!(last["status"], "ok");
    let usage = lines
        .iter()
        .find(|l| l["type"] == "usage")
        .expect("usage line");
    for field in [
        "input",
        "output",
        "cache_read",
        "cache_write",
        "cache_write_1h",
        "cost",
    ] {
        assert!(usage.get(field).is_some(), "{field} present: {usage}");
    }
    assert_eq!(usage["cache_read"], 1000);
}

// Verifies: FR-PROV-6 (with no provider extension enabled the agent reports
// that no model is available and offers the install command), FR-PROV-9
// (disabling the default provider leaves zero providers, an ordinary
// state, not a crash)
#[test]
fn disabled_provider_reports_the_install_command() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("never called"))]));
    let box_ = sandbox("no-provider");
    let output = box_.run_env(
        Some(&mock),
        &["-p", "hi"],
        &[("LCA_PROVIDER", "disabled-extension")],
    );
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("No model is available"), "{text}");
    assert!(
        text.contains("lca ext install"),
        "offers the install command: {text}"
    );
    assert_eq!(mock.request_count(), 0, "no request without a provider");
}

// Verifies: FR-PROV-9's disable path proper (FR-PERM-19's storage) -
// the default provider is registered but the grant store has it
// disabled for this project, so zero providers are enabled, the agent
// reports FR-PROV-6's message, and no socket opens.
#[test]
fn a_grant_store_disable_leaves_zero_enabled_providers() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("never called"))]));
    let box_ = sandbox("provider-off");
    box_.approve_loopback_net(serde_json::json!({
        "extensions": { "openai-compatible": false },
    }));
    let output = box_.run(Some(&mock), &["-p", "hi"]);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("No model is available"), "{text}");
    assert!(text.contains("lca ext install"), "{text}");
    assert_eq!(
        mock.request_count(),
        0,
        "a disabled provider opens no socket"
    );
}

// Exit code table, docs/headless.md:2 = usage error.
#[test]
fn bad_flags_exit_two() {
    let box_ = sandbox("usage");
    let output = box_.run(None, &["--definitely-not-a-flag"]);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
}

// Exit3: provider error (after the retry limit for retryable classes).
#[test]
fn provider_error_exits_three() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Status(401)]));
    let box_ = sandbox("provider-error");
    let output = box_.run(Some(&mock), &["-p", "hi", "--json"]);
    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let error = lines
        .iter()
        .find(|l| l["type"] == "error")
        .expect("error envelope");
    assert_eq!(error["class"], "auth");
    assert_eq!(error["retryable"], false);
    let last = lines.last().expect("turn-end");
    assert_eq!(last["type"], "turn-end");
    assert_eq!(last["status"], "error");
}

// Exit4: an action needed approval and headless mode cannot prompt
// (FR-TOOL-3's headless path).
#[test]
fn unapprovable_action_exits_four() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("shell", r#"{"command":"rm -rf /"}"#)),
        Reply::Sse(sse_text("understood")),
    ]));
    let box_ = sandbox("denied");
    let output = box_.run(Some(&mock), &["-p", "clean up", "--json"]);
    assert_eq!(output.status.code(), Some(4), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let result = lines
        .iter()
        .find(|l| l["type"] == "tool-result")
        .expect("tool-result envelope");
    assert_eq!(result["status"], "denied");
    assert_eq!(
        mock.request_count(),
        2,
        "the model continued after the denial"
    );
}

// Exit5: the iteration limit aborted the turn (FR-CORE-9).
#[test]
fn iteration_limit_exits_five() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("write", r#"{"path":"a.txt","content":"A"}"#)),
        Reply::Sse(sse_tool_call("write", r#"{"path":"b.txt","content":"B"}"#)),
        Reply::Sse(sse_text("never")),
    ]));
    let box_ = sandbox("iteration");
    let output = box_.run_env(
        Some(&mock),
        &["-p", "loop", "--json"],
        &[("LCA_TOOL_MAX_ITERATIONS", "1")],
    );
    assert_eq!(output.status.code(), Some(5), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let last = lines.last().expect("turn-end");
    assert_eq!(last["stop_reason"], "iteration-limit");
    assert!(
        !box_.project().join("b.txt").exists(),
        "second round never ran"
    );
}

// Exit6: the named session is missing (docs/headless.md).
#[test]
fn missing_session_exits_six() {
    let box_ = sandbox("session-missing");
    let output = box_.run(None, &["export", "no-such-session"]);
    assert_eq!(output.status.code(), Some(6), "stderr: {}", stderr(&output));
}

// Verifies: FR-SESS-8 (R7) - a run's state lands in `$HOME/.lca`, and the
// platform-conventional directories the old build used are not touched.
#[test]
fn state_lives_in_the_home_dot_directory() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("stored"))]));
    let box_ = sandbox("home-dot-dir");
    let output = box_.run(Some(&mock), &["-p", "hi"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));

    let home_dot = box_.home.join(".lca");
    assert!(
        home_dot.join("sessions").is_dir(),
        "sessions live in $HOME/.lca: {home_dot:?}"
    );
    assert!(
        home_dot.join("grants.json").is_file() || home_dot.join("sessions").is_dir(),
        "the agent's tree is under $HOME/.lca"
    );
    // The old homes stay empty: no migration code exists, by design.
    let legacy = box_.data.join("lca");
    assert!(
        !legacy.exists(),
        "the old platform data directory is not written: {legacy:?}"
    );
}

// A run writes its session, `resume` lists it (FR-SESS-2), and rename shows
// up in that list.
#[test]
fn sessions_persist_and_resume_lists_them() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("stored"))]));
    let box_ = sandbox("resume");
    let output = box_.run(Some(&mock), &["-p", "hi"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));

    let output = box_.run(None, &["resume"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let listing = stdout(&output);
    assert!(!listing.is_empty(), "resume lists sessions: {listing:?}");

    let id = listing
        .lines()
        .next()
        .expect("one session at least")
        .split_whitespace()
        .next()
        .expect("session id")
        .to_string();

    let output = box_.run(None, &["rename", &id, "renamed title"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let output = box_.run(None, &["resume"]);
    assert!(
        stdout(&output).contains("renamed title"),
        "listing: {}",
        stdout(&output)
    );
}

// Verifies: D5 — `lca session gc <id>` runs against a real session and
// reports a clean tree when nothing is orphaned.
#[test]
fn session_gc_reports_when_nothing_is_orphaned() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("stored"))]));
    let box_ = sandbox("session-gc");
    let output = box_.run(Some(&mock), &["-p", "hi"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));

    let listing = stdout(&box_.run(None, &["resume"]));
    let id = listing
        .lines()
        .next()
        .expect("one session at least")
        .split_whitespace()
        .next()
        .expect("session id")
        .to_string();
    let output = box_.run(None, &["session", "gc", &id]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("no unreferenced attachments"),
        "stdout: {}",
        stdout(&output)
    );
}

// Verifies: ADR-0029 - `--attach` stages an image into the session store, the
// user record carries the content hash, and the model-visible stub rides in
// the message text (so a provider without vision still sees the image exists).
#[test]
fn attach_flag_stages_an_image_on_the_user_record() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("seen"))]));
    let box_ = sandbox("attach");
    let png = box_.project().join("shot.png");
    std::fs::write(
        &png,
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3],
    )
    .expect("write image");
    let output = box_.run(
        Some(&mock),
        &["-p", "look", "--attach", png.to_str().expect("utf-8 path")],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let log = find_session_log(&box_.state_dir()).expect("a session log");
    let text = std::fs::read_to_string(log).expect("read log");
    assert!(
        text.contains("[image attachment"),
        "the stub text is on the user record: {text}"
    );
    assert!(
        text.contains("\"attachments\":[\""),
        "the content hash is on the user record: {text}"
    );
}

// Verifies: FR-CFG-2 (the config command prints each resolved value and the
// source that set it)
#[test]
fn config_prints_values_with_sources() {
    let box_ = sandbox("config");
    let output = box_.run(None, &["config"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("provider"), "{text}");
    assert!(text.contains("default"), "sources are named: {text}");

    let output = box_.run_env(None, &["config"], &[("LCA_TOOL_TIMEOUT_SECONDS", "7")]);
    let text = stdout(&output);
    assert!(text.contains("tool.timeout_seconds = 7"), "{text}");
    assert!(text.contains("environment"), "source named: {text}");
}

// Release policy + ABI policy: --version prints the agent version, the ABI
// version, the crate version, and the build target.
#[test]
fn version_prints_all_four_facts() {
    let box_ = sandbox("version");
    let output = box_.run(None, &["--version"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(
        text.contains("abi 0.6"),
        "the window's live ABI line: {text}"
    );
    assert!(text.contains("target"), "{text}");
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
}

// ---------------------------------------------------------------------------
// Phase 5 exit: a clean machine installs from an OCI reference and a
// plain HTTPS archive, sees each consent screen, approves both, and
// runs a turn (FR-DIST-1/2/5/6/9, the capability catalog's consent
// surface, FR-EXT-9's denial count along the way).
//
// Fixtures: extensions/openai-compatible/fixtures/component.wasm and
// extensions/skills/fixtures/component.wasm, rebuilt with
// `cargo build -p <crate> --target wasm32-wasip2 --release` and copied
// into place (same convention as conformance's).
// ---------------------------------------------------------------------------

// Verifies: #152 (a grant-store write failure warns into the log file in
// every run, and stderr only under `--verbose`). `--yolo` auto-approves
// the shell call, so the `always` persist hits the read-only store.
#[test]
fn verbose_routes_store_warnings_to_stderr() {
    for verbose in [false, true] {
        let runtime = rt();
        let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_tool_call(
            "shell",
            r#"{"command":"echo hi"}"#,
        ))]));
        let box_ = sandbox(&format!("verbose-{verbose}"));
        // Untrusted: folder trust would auto-approve the workspace shell
        // call with no store write to fail. The sabotage is a directory
        // at the atomic-save temp path, so opens and session writes work
        // while every grant persist fails.
        box_.approve_loopback_net(serde_json::json!({"trusted": false}));
        std::fs::create_dir(box_.state_dir().join("grants.json.tmp")).expect("sabotage");
        let mut args = vec!["--yolo", "-p", "run it"];
        if verbose {
            args.push("--verbose");
        }
        let output = box_.run(Some(&mock), &args);
        let err = stderr(&output);
        assert_eq!(
            output.status.code(),
            Some(1),
            "the turn fails loudly (verbose={verbose}): {err}"
        );
        let log = std::fs::read_to_string(box_.state_dir().join("logs/lca.log"))
            .expect("the file log exists in both modes");
        assert!(
            log.contains("permission store"),
            "the file log records the warning (verbose={verbose}): {err}"
        );
        assert_eq!(
            err.contains("permission store"),
            verbose,
            "--verbose routes the warning to stderr, quiet stays quiet (verbose={verbose}): {err}"
        );
    }
}

// Gh #124's acceptance: a headless run answers a tool's `ui.confirm`
// with the denied value - no modal, no hang, exit 0 with the verdict
// as the tool result.
#[test]
fn headless_answers_a_tool_dialog_with_the_denied_value() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("conformance", r#"{"mode":"ask-confirm"}"#)),
        Reply::Sse(sse_text("noted")),
    ]));
    let box_ = sandbox("headless-dialog");
    box_.install_component(
        "conformance",
        include_str!("../../../extensions/conformance/extension.toml"),
        include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm"),
    );
    let output = box_.run(Some(&mock), &["-p", "ask the question", "--json"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let result = lines
        .iter()
        .find(|l| l["type"] == "tool-result")
        .expect("tool-result envelope");
    assert_eq!(result["status"], "ok");
    assert!(
        result["content"]
            .as_str()
            .unwrap_or_default()
            .contains("confirm: false"),
        "the denied verdict crossed: {result}"
    );
}

// Verifies: gh #77 (the headless nested-call turn, mock orchestrator) -
// a tool calling another tool through the host carries `<parent>/<n>`
// ids with `parent_call_id` on every event, and the parent's result
// keeps the bounded nested record.
#[test]
fn headless_nested_calls_carry_parent_ids_and_bounded_records() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call(
            "conformance",
            r#"{"mode":"nested","target":"conformance-deferred"}"#,
        )),
        Reply::Sse(sse_text("nested done")),
    ]));
    let box_ = sandbox("headless-nested");
    box_.install_component(
        "conformance",
        include_str!("../../../extensions/conformance/extension.toml"),
        include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm"),
    );
    let output = box_.run(Some(&mock), &["-p", "nest once", "--json"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let calls: Vec<&serde_json::Value> =
        lines.iter().filter(|l| l["type"] == "tool-call").collect();
    assert_eq!(calls.len(), 2, "parent plus one nested: {calls:?}");
    assert_eq!(calls[0]["name"], "conformance");
    assert_eq!(calls[0]["parent_call_id"], serde_json::Value::Null);
    assert_eq!(calls[1]["call_id"], "call-e2e/1");
    assert_eq!(calls[1]["parent_call_id"], "call-e2e");
    let results: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|l| l["type"] == "tool-result")
        .collect();
    assert_eq!(results.len(), 2);
    let parent = results
        .iter()
        .find(|r| r["call_id"] == "call-e2e")
        .expect("parent result");
    assert_eq!(parent["status"], "ok");
    let nested = parent["nested"].as_array().expect("bounded record");
    assert_eq!(nested.len(), 1, "one nested entry: {nested:?}");
    assert_eq!(nested[0]["name"], "conformance-deferred");
    assert!(
        nested[0]["content_head"]
            .as_str()
            .unwrap_or_default()
            .contains("conformance ok"),
        "the nested outcome recorded: {nested:?}"
    );
    // The session log agrees with the envelopes.
    let log_path = find_session_log(&box_.state_dir()).expect("session log");
    let log = std::fs::read_to_string(&log_path).expect("read log");
    assert!(
        log.contains("conformance-deferred") && log.contains("nested"),
        "the bounded record persisted"
    );
}

// Verifies: gh #45 (the hook-veto journey) - a redaction hook mutates
// a result live: the model sees the rewritten text, never the token.
#[test]
fn headless_redaction_hook_mutates_the_result_live() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_tool_call("conformance", r#"{"mode":"spill-secret"}"#)),
        Reply::Sse(sse_text("noted")),
    ]));
    let box_ = sandbox("headless-redact");
    box_.install_component(
        "conformance",
        include_str!("../../../extensions/conformance/extension.toml"),
        include_bytes!("../../../extensions/conformance/fixtures/tool-world.wasm"),
    );
    let output = box_.run(Some(&mock), &["-p", "spill it", "--json"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let result = lines
        .iter()
        .find(|l| l["type"] == "tool-result")
        .expect("tool-result envelope");
    let content = result["content"].as_str().unwrap_or_default();
    assert!(
        content.contains("conformance-redacted"),
        "the hook rewrote the token: {content}"
    );
    assert!(
        !content.contains("conformance-secret"),
        "the raw token never reached the model: {content}"
    );
}
