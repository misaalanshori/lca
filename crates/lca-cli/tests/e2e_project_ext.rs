//! Project-local extension tests (gh #138): a trusted project's
//! `.lca/extensions/` component loads and serves; an untrusted
//! project's is ignored.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::*;

/// Lay a project-local provider under its own name (renamed from the
/// fixture so the bundled native of the fixture's name cannot serve
/// either run: whatever answers, it is the project component or
/// nothing).
fn lay_project_extension(box_: &Sandbox) {
    let manifest = OPENAI_MANIFEST
        .replace("name = \"openai-compatible\"", "name = \"project-llm\"")
        .replace(
            "namespace = \"openai-compatible\"",
            "namespace = \"project-llm\"",
        );
    let dir = box_
        .project()
        .join(".lca")
        .join("extensions")
        .join("project-llm");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("extension.toml"), manifest).expect("manifest");
    std::fs::write(dir.join("component.wasm"), OPENAI_COMPONENT).expect("component");
}

/// Grants trusting nothing: loopback net only, so a turn that needs a
/// provider fails for lack of one rather than for lack of net.
fn write_untrusted_grants(box_: &Sandbox) {
    let project = std::fs::canonicalize(box_.project()).expect("canonical project");
    let grants = serde_json::json!({
        "version": 1,
        "projects": { project.to_string_lossy(): { "net_patterns": ["127.0.0.1"] } },
    });
    std::fs::create_dir_all(box_.state_dir()).expect("mkdir lca");
    std::fs::write(
        box_.state_dir().join("grants.json"),
        serde_json::to_vec_pretty(&grants).expect("grants serialize"),
    )
    .expect("write grants");
}

// Verifies: gh #138 — a trusted project's own provider component
// loads and serves the turn from its credential namespace; the same
// layout in an untrusted project is ignored, so `--provider` names
// nothing and the run fails.
#[test]
fn trusted_project_extension_serves_and_untrusted_is_ignored() {
    let runtime = rt();
    let model = runtime.block_on(start_mock(vec![Reply::Sse(sse_text(
        "project-local and chatting",
    ))]));

    // --- Trusted: the project component serves.
    let trusted = sandbox("project-ext-trusted");
    lay_project_extension(&trusted);
    trusted.write_grants(true);
    trusted.write_credentials(
        "project-llm",
        serde_json::json!({ "api_key": "test-key", "base_url": model.url() }),
    );
    // Provider selection is the config `provider` (env `LCA_PROVIDER`);
    // `--provider` only scopes `--model` inside one, so it cannot
    // select here.
    let output = trusted.run_env(
        Some(&model),
        &["--model", "zen-free", "-p", "hi", "--json"],
        &[("OPENAI_BASE_URL", ""), ("LCA_PROVIDER", "project-llm")],
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let lines = json_lines(&output);
    let text_line = lines
        .iter()
        .find(|l| l["type"] == "text")
        .expect("a text envelope");
    assert_eq!(text_line["content"], "project-local and chatting");
    assert_eq!(model.request_count(), 1, "exactly one model call");

    // --- Untrusted: the same layout is ignored, so the configured
    // provider names nothing and the run fails instead of serving.
    let untrusted = sandbox("project-ext-untrusted");
    lay_project_extension(&untrusted);
    write_untrusted_grants(&untrusted);
    untrusted.write_credentials(
        "project-llm",
        serde_json::json!({ "api_key": "test-key", "base_url": model.url() }),
    );
    let output = untrusted.run_env(
        Some(&model),
        &["--model", "zen-free", "-p", "hi", "--json"],
        &[("OPENAI_BASE_URL", ""), ("LCA_PROVIDER", "project-llm")],
    );
    assert_ne!(
        output.status.code(),
        Some(0),
        "nothing serves without trust: {}",
        stderr(&output)
    );
}
