//! gh #418 Phase C#C1: a stdio entry's `env` and `cwd`, and the stderr tail
//! that explains a server that died.
//!
//! These rows drive the real seam: `mcp.json` text through
//! [`lca_mcp::config::parse_mcp_json`], an entry through
//! [`lca_mcp::McpManager::load`], a real `python3` child spawned by
//! `lca-tools`' `process` capability, and the fixture's answer read back
//! through the bridge. Nothing here mocks the spawn, because the whole point
//! of the change is what the child actually receives.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_mcp::config::{McpExposure, ServerEntry, ServerSource, parse_mcp_json};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};

struct Allow;

impl PermissionPrompt for Allow {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// A scratch root with the five scope directories the engine needs.
fn root(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(&format!("lca-mcp-c1-{name}"));
    for part in [
        "workspace",
        "workspace/subdir",
        "private",
        "config",
        "data",
        "tmp",
    ] {
        std::fs::create_dir_all(dir.join(part)).expect("mkdir");
    }
    dir
}

fn engine(root: &std::path::Path) -> Arc<lca_tools::Capabilities> {
    Arc::new(lca_tools::Capabilities::new(
        "mcp",
        lca_mcp::manifest_grants(),
        ScopeRoots {
            workspace: root.join("workspace"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        )),
        root.join("workspace"),
        None,
    ))
}

/// Parse one entry from `mcp.json` text, expecting it to load.
fn entry(text: &str) -> ServerEntry {
    let parsed = parse_mcp_json(text, ServerSource::User);
    assert_eq!(parsed.warnings, Vec::<String>::new(), "no warnings");
    assert_eq!(parsed.servers.len(), 1);
    parsed.servers.into_iter().next().expect("one entry")
}

/// Connect one entry through the manager and call its `probe` tool.
async fn probe(root: &std::path::Path, entry: ServerEntry) -> Result<serde_json::Value, String> {
    let manager = lca_mcp::McpManager::load(
        engine(root),
        root.join("data").join("mcp.json"),
        None,
        vec![entry],
    );
    let bridge = manager.bridge();
    let statuses = bridge.statuses();
    let status = statuses
        .first()
        .ok_or_else(|| "no server row".to_string())?;
    if let lca_mcp::ServerStateKind::Failed(err) = &status.state {
        return Err(err.clone());
    }
    assert!(
        matches!(status.state, lca_mcp::ServerStateKind::Connected),
        "the probe server connects: {:?}",
        status.state
    );
    let call = lca_protocol::ToolCall {
        call_id: "c1".to_string(),
        name: "mcp__probe__probe".to_string(),
        arguments: "{}".to_string(),
        parent_call_id: None,
    };
    let result = bridge
        .execute_tool(&call)
        .await
        .map_err(|err| format!("{err:?}"))?;
    let text = result.content.clone();
    serde_json::from_str(&text).map_err(|err| format!("probe answer is not JSON: {err}: {text}"))
}

// Verifies: gh #418 Phase C#C1 - `env` values expand `${VAR}` and are passed
// to the child *alongside* the inherited environment (pi's rule: `env`
// overlays `process.env`, it never replaces it). Receipt: the child reports
// both probe variables and `PATH` present.
#[tokio::test]
async fn a_stdio_env_overlays_the_inherited_environment() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    // SAFETY: test-scoped name, set before the spawn reads it.
    unsafe {
        std::env::set_var("LCA_C1_FROM_PARENT", "parent-value");
        std::env::set_var("LCA_C1_EXPANDED", "expanded-value");
    }
    let root = root("env");
    let text = format!(
        r#"{{"mcpServers": {{"probe": {{
            "command": "python3",
            "args": ["{}"],
            "env": {{
                "LCA_C1_LITERAL": "literal-value",
                "LCA_C1_FROM_CONFIG": "${{LCA_C1_EXPANDED}}",
                "LCA_PROBE_ENV": "LCA_C1_LITERAL,LCA_C1_FROM_CONFIG,LCA_C1_FROM_PARENT"
            }}
        }}}}}}"#,
        fixture("probe-mcp-server.py")
            .to_string_lossy()
            .replace('\\', "/")
    );
    let answer = probe(&root, entry(&text))
        .await
        .expect("the probe server answers");
    let env = answer.get("env").expect("env reported");
    assert_eq!(
        env.get("LCA_C1_LITERAL").and_then(|v| v.as_str()),
        Some("literal-value"),
        "a literal env value reaches the child: {answer}"
    );
    assert_eq!(
        env.get("LCA_C1_FROM_CONFIG").and_then(|v| v.as_str()),
        Some("expanded-value"),
        "`${{VAR}}` expands in env values: {answer}"
    );
    assert_eq!(
        env.get("LCA_C1_FROM_PARENT").and_then(|v| v.as_str()),
        Some("parent-value"),
        "the inherited environment survives the overlay: {answer}"
    );
    assert_eq!(
        answer.get("path_present").and_then(|v| v.as_bool()),
        Some(true),
        "PATH must survive, or every server that shells out breaks: {answer}"
    );
}

// Verifies: gh #418 Phase C#C1 - `cwd` is a directory path resolved against
// the session cwd (pi: `resolve(cwd, expandHome(config.cwd ?? "."))`), not a
// capability scope name. Receipt: the child reports the directory it landed
// in.
#[tokio::test]
async fn a_stdio_cwd_is_a_path_resolved_against_the_session_cwd() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = root("cwd");
    let script = fixture("probe-mcp-server.py")
        .to_string_lossy()
        .replace('\\', "/");
    // A relative path: `./subdir` under the workspace (the session cwd).
    let text = format!(
        r#"{{"mcpServers": {{"probe": {{
            "command": "python3",
            "args": ["{script}"],
            "cwd": "./subdir"
        }}}}}}"#
    );
    let answer = probe(&root, entry(&text))
        .await
        .expect("the probe server answers");
    let cwd = answer.get("cwd").and_then(|v| v.as_str()).expect("cwd");
    let expected = std::fs::canonicalize(root.join("workspace").join("subdir"))
        .expect("canonical workspace/subdir");
    assert_eq!(
        std::fs::canonicalize(cwd).expect("canonical reported cwd"),
        expected,
        "the child landed in the resolved directory: {answer}"
    );

    // No `cwd` at all defaults to the session cwd.
    let bare =
        format!(r#"{{"mcpServers": {{"probe": {{"command": "python3", "args": ["{script}"]}}}}}}"#);
    let answer = probe(&root, entry(&bare))
        .await
        .expect("the probe server answers");
    let cwd = answer.get("cwd").and_then(|v| v.as_str()).expect("cwd");
    assert_eq!(
        std::fs::canonicalize(cwd).expect("canonical"),
        std::fs::canonicalize(root.join("workspace")).expect("canonical workspace"),
        "an absent cwd means the session cwd: {answer}"
    );
}

// Verifies: gh #418 Phase C#C1 - a server that writes to stderr and exits
// surfaces that tail in the failure, so `/mcp` can say *why* it died instead
// of only that it did.
#[tokio::test]
async fn a_failed_stdio_start_carries_the_stderr_tail() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = root("stderr");
    let text = format!(
        r#"{{"mcpServers": {{"probe": {{
            "command": "python3",
            "args": ["{}"],
            "env": {{"LCA_PROBE_FAIL": "1"}}
        }}}}}}"#,
        fixture("probe-mcp-server.py")
            .to_string_lossy()
            .replace('\\', "/")
    );
    let err = probe(&root, entry(&text))
        .await
        .expect_err("the server dies before answering");
    assert!(
        err.contains("the config is wrong") || err.contains("hint: check LCA_PROBE_ENV"),
        "the failure carries the child's stderr tail: {err}"
    );
}

// Verifies: gh #418 Phase C#C1 - the tail is bounded, and a clean start says
// nothing extra. A server that never writes to stderr produces a failure
// message with no tail section (rather than an empty one).
#[tokio::test]
async fn a_failure_without_stderr_output_stays_clean() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = root("no-stderr");
    // A command that does not exist: the spawn itself fails, and there is
    // no child to have written anything.
    let text = r#"{"mcpServers": {"probe": {"command": "definitely-not-a-real-program-c1"}}}"#;
    let err = probe(&root, entry(text))
        .await
        .expect_err("the spawn fails");
    assert!(
        !err.contains("stderr:"),
        "no invented tail when the child never ran: {err}"
    );
}

// Verifies: gh #418 Phase C#C1 - `cwd` still cannot escape the workspace. The
// path is resolved through the `workspace` scope, so `..` and an absolute
// path are refused rather than silently clamped.
#[tokio::test]
async fn a_stdio_cwd_cannot_escape_the_workspace() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = root("escape");
    let script = fixture("probe-mcp-server.py")
        .to_string_lossy()
        .replace('\\', "/");
    let text = format!(
        r#"{{"mcpServers": {{"probe": {{
            "command": "python3",
            "args": ["{script}"],
            "cwd": "../../etc"
        }}}}}}"#
    );
    let err = probe(&root, entry(&text))
        .await
        .expect_err("the escape is refused");
    assert!(
        err.to_lowercase().contains("escape") || err.to_lowercase().contains("scope"),
        "the refusal names the scope violation: {err}"
    );
}

// Verifies: gh #418 Phase C#C1 - `env` and `cwd` are fields of the entry the
// manager persists, so an edit round-trip keeps them (they used to be
// dropped from the written shape, which would have deleted a user's `env`
// block on the next `/mcp` verb).
#[tokio::test]
async fn the_persisted_entry_keeps_env_and_cwd() {
    let parsed = parse_mcp_json(
        r#"{"mcpServers": {"probe": {"command": "python3",
            "args": ["x.py"],
            "env": {"K": "v"},
            "cwd": "./subdir",
            "exposure": "direct"}}}"#,
        ServerSource::User,
    );
    assert_eq!(parsed.warnings, Vec::<String>::new());
    let entry = parsed.servers.into_iter().next().expect("one");
    let lca_mcp::config::EntryKind::Stdio { env, cwd, .. } = &entry.kind else {
        panic!("stdio");
    };
    assert_eq!(env.get("K").map(String::as_str), Some("v"));
    assert_eq!(cwd.as_deref(), Some("./subdir"));
    assert_eq!(entry.exposure, McpExposure::Direct);
}
