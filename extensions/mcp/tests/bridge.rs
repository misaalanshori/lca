//! gh #53 phase 1: the bridge connects a stdio MCP server through the
//! `process` capability, lists its tools under pi's `mcp__<server>__<tool>`
//! names, calls one, and records a spawn denial.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/echo-mcp-server.py")
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn scratch(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(name);
    for part in ["project", "workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(dir.join(part)).expect("mkdir");
    }
    dir
}

struct Answer {
    asked: Mutex<Vec<String>>,
    answer: Decision,
}

impl PermissionPrompt for Answer {
    fn ask(&mut self, action: &lca_permissions::Action) -> Decision {
        self.asked.lock().expect("spy").push(action.display());
        self.answer
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

/// The engine plus its prompt spy: the permission-path assertions
/// read `asked` back to prove the spawn went through the prompt.
fn engine(name: &str, answer: Decision) -> (Arc<lca_tools::Capabilities>, Arc<Mutex<Answer>>) {
    let root = scratch(name);
    let spy = Arc::new(Mutex::new(Answer {
        asked: Mutex::new(Vec::new()),
        answer,
    }));
    let caps = Arc::new(lca_tools::Capabilities::new(
        "mcp",
        mcp::manifest_grants(),
        ScopeRoots {
            workspace: root.join("workspace"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        spy.clone(),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        )),
        root.join("workspace"),
        None,
    ));
    (caps, spy)
}

fn echo_server() -> mcp::ServerConfig {
    mcp::ServerConfig {
        name: "echo".to_string(),
        command: "python3".to_string(),
        args: vec![fixture().to_string_lossy().into_owned()],
        cwd_scope: "workspace".to_string(),
    }
}

// Verifies: gh #53 - the echo fixture's one tool lists as a direct
// tool under its pi name, carrying the server's read-only hint.
#[test]
fn an_echo_server_lists_one_direct_tool() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let (caps, spy) = engine("mcp-list", Decision::Always);
    let bridge = mcp::McpBridge::connect(caps, vec![echo_server()]).expect("connect");
    assert!(
        spy.lock()
            .expect("spy")
            .asked
            .lock()
            .expect("asked")
            .iter()
            .any(|action| action.contains("python3")),
        "the spawn asked the permission layer"
    );
    let specs = bridge.tool_specs().expect("specs");
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "mcp__echo__echo");
    assert_eq!(specs[0].description, "Echoes its text argument back.");
    assert_eq!(specs[0].exposure, lca_protocol::ToolExposure::Direct);
    assert_eq!(
        specs[0]
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.read_only_hint),
        Some(true),
        "the server's read-only hint survives the crossing"
    );
}

// Verifies: gh #53 - a call round-trips through the server and the
// server's answer is the tool result.
#[tokio::test]
async fn a_call_returns_the_servers_answer() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let (caps, _spy) = engine("mcp-call", Decision::Always);
    let bridge = mcp::McpBridge::connect(caps, vec![echo_server()]).expect("connect");
    let result = bridge
        .execute_tool(&lca_protocol::ToolCall {
            call_id: "call-1".to_string(),
            name: "mcp__echo__echo".to_string(),
            arguments: r#"{"text":"hello"}"#.to_string(),
            parent_call_id: None,
        })
        .await
        .expect("run");
    assert_eq!(result.status, lca_protocol::ToolResultStatus::Ok);
    assert_eq!(result.content, "echo: hello");
}

// Verifies: gh #53 - tool names follow pi's `mcp__<server>__<tool>`
// rule: every character outside letters, digits, and `_` becomes `_`.
#[test]
fn tool_names_sanitize_pi_style() {
    assert_eq!(mcp::tool_name("echo", "echo"), "mcp__echo__echo");
    assert_eq!(
        mcp::tool_name("my-server", "do.thing"),
        "mcp__my_server__do_thing"
    );
}

// Verifies: gh #53 - a declined spawn never starts the server, the
// denial is recorded on the engine, and the journal keeps the row.
#[test]
fn a_denied_spawn_is_recorded() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = scratch("mcp-deny");
    let (caps, spy) = engine("mcp-deny", Decision::Denied);
    let err = match mcp::McpBridge::connect(caps.clone(), vec![echo_server()]) {
        Ok(_) => panic!("a denied spawn refuses the bridge"),
        Err(err) => err,
    };
    assert!(
        spy.lock()
            .expect("spy")
            .asked
            .lock()
            .expect("asked")
            .iter()
            .any(|action| action.contains("python3")),
        "the refused spawn still went through the prompt"
    );
    assert!(
        err.contains("declined") || err.contains("denied"),
        "the error names the denial: {err}"
    );
    let denials = caps.denials();
    assert!(
        denials.iter().any(|denial| denial.capability == "process"),
        "the process denial is recorded: {denials:?}"
    );
    let journal = root.join("data/extensions/mcp/denials.jsonl");
    assert!(journal.exists(), "the denial journal keeps the row");
}
