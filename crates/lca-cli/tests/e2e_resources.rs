//! GitHub #70, live: per-run resource flags against the loopback mock
//! (no real network, no real credentials).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

use common::{Reply, rt, sandbox, sse_text, start_mock};

fn tool_names(mock: &common::Mock) -> Vec<String> {
    mock.bodies()
        .iter()
        .filter_map(|body| serde_json::from_str::<serde_json::Value>(body).ok())
        .filter(|body| body.get("messages").is_some())
        .flat_map(|body| {
            body["tools"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .collect()
}

// Verifies: gh #70 - `lca -e tool-world.wasm -p x` declares the
// guest's tool; `--no-extensions` keeps the explicit guest while
// dropping everything else.
#[test]
fn extra_extension_declares_its_tool() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![
        Reply::Sse(sse_text("with ext")),
        Reply::Sse(sse_text("without ext")),
    ]));
    let box_ = sandbox("headless-extra-ext");
    let dir = box_.state_dir().join("ext");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../extensions/conformance/fixtures/tool-world.wasm"
        ),
        dir.join("tool-world.wasm"),
    )
    .expect("stage fixture");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../extensions/conformance/extension.toml"
        ),
        dir.join("extension.toml"),
    )
    .expect("stage manifest");

    let with = box_.run(
        Some(&mock),
        &[
            "--model",
            "zen-free",
            "-e",
            dir.join("tool-world.wasm").to_str().expect("path"),
            "-p",
            "hi",
        ],
    );
    assert_eq!(
        with.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&with.stderr)
    );
    let names = tool_names(&mock);
    assert!(
        names.contains(&"conformance".to_string()),
        "the guest tool declares: {names:?}"
    );

    let without = box_.run(
        Some(&mock),
        &[
            "--model",
            "zen-free",
            "-e",
            dir.join("tool-world.wasm").to_str().expect("path"),
            "--no-extensions",
            "-p",
            "hi",
        ],
    );
    assert_eq!(
        without.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&without.stderr)
    );
    let names = tool_names(&mock);
    assert!(
        names.iter().filter(|name| *name == "conformance").count() >= 1,
        "explicit -e survives --no-extensions: {names:?}"
    );
    // The second request (the `--no-extensions` run) carries only
    // built-ins plus the explicit guest: group the names per request.
    let per_request: Vec<Vec<String>> = mock
        .bodies()
        .iter()
        .filter_map(|body| serde_json::from_str::<serde_json::Value>(body).ok())
        .filter(|body| body.get("messages").is_some())
        .map(|body| {
            body["tools"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .collect();
    assert_eq!(per_request.len(), 2, "one turn each");
    let builtins = [
        "read", "write", "edit", "list", "glob", "grep", "shell", "skill",
    ];
    for name in &per_request[1] {
        assert!(
            builtins.contains(&name.as_str()) || name == "conformance" || name == "tool_search",
            "only built-ins survive --no-extensions: {name}"
        );
    }
}

// Verifies: gh #70 - `--skill` loads a skill file into the prompt
// catalog with CLI attribution.
#[test]
fn skill_flag_loads_into_the_catalog() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("skilled"))]));
    let box_ = sandbox("headless-skill");
    let dir = box_.state_dir().join("skills");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("demo.md"), "# demo\nDoes demos.\n").expect("write");
    let run = box_.run(
        Some(&mock),
        &[
            "--model",
            "zen-free",
            "--skill",
            dir.join("demo.md").to_str().expect("path"),
            "-p",
            "hi",
        ],
    );
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let bodies = mock.bodies().join("\n");
    assert!(
        bodies.contains("demo") && bodies.contains("command line"),
        "the catalog names the skill and its source"
    );
}

// Verifies: gh #70 - a missing resource path refuses before routing.
#[test]
fn missing_resource_paths_refuse() {
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![]));
    let box_ = sandbox("headless-bad-resource");
    let run = box_.run(
        Some(&mock),
        &["--model", "zen-free", "--skill", "nope.md", "-p", "hi"],
    );
    assert_eq!(run.status.code(), Some(2), "usage error");
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("--skill"),
        "names the flag: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}
