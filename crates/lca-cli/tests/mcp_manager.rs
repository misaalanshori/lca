//! gh #53 phase 3: the host manager - trust-gated loading, verbs,
//! persistence, and the prompt section (the echo fixture drives the
//! live paths).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_cli::mcp::{McpManager, load_entries, servers_section};

fn root(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(&format!("lca-mcp-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    for part in ["project", "project/.lca", "data"] {
        std::fs::create_dir_all(dir.join(part)).expect("mkdir");
    }
    dir
}

fn fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../extensions/mcp/fixtures/echo-mcp-server.py")
        .to_string_lossy()
        .into_owned()
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

struct Always;
impl lca_permissions::PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Always
    }
    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

fn engine(name: &str) -> Arc<lca_tools::Capabilities> {
    let dir = lca_testkit::scratch_path(&format!("lca-mcp-eng-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    for part in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(dir.join(part)).expect("mkdir");
    }
    let mut grants = mcp::manifest_grants();
    grants.oauth = Some(lca_permissions::OAuthSettings {
        redirect_path: "/callback".to_string(),
        timeout_seconds: 30,
    });
    grants.credentials = true;
    Arc::new(lca_tools::Capabilities::new(
        "mcp",
        grants,
        lca_permissions::ScopeRoots {
            workspace: dir.join("project"),
            private: dir.join("private"),
            home_config: dir.join("config"),
            temp: dir.join("tmp"),
            state_dir: dir.join("data"),
        },
        Arc::new(Mutex::new(Always)),
        Arc::new(Mutex::new(
            lca_permissions::GrantStore::open(&dir.join("grants.json")).expect("grants"),
        )),
        dir.join("project"),
        None,
    ))
}

fn write(path: &PathBuf, text: &str) {
    std::fs::write(path, text).expect("write");
}

// Verifies: gh #53 - user entries load; the trusted project's patch
// the user file; the untrusted project's are ignored silently.
#[test]
fn project_files_load_only_when_trusted() {
    let dir = root("trust");
    write(
        &dir.join("data/mcp.json"),
        &format!(
            r#"{{"mcpServers": {{"echo": {{"command": "python3", "args": ["{f}"], "exposure": "direct"}}}}}}"#,
            f = fixture()
        ),
    );
    write(
        &dir.join("project/.lca/mcp.json"),
        r#"{"mcpServers": {"echo": {"enabled": false}}}"#,
    );
    let project = dir.join("project");
    let data = dir.join("data");

    let untrusted = load_entries(&data, &project, false);
    assert_eq!(untrusted.entries.len(), 1);
    assert!(untrusted.entries[0].enabled, "untrusted project ignored");

    let trusted = load_entries(&data, &project, true);
    assert_eq!(trusted.entries.len(), 1);
    assert!(!trusted.entries[0].enabled, "trusted project patches");
    assert_eq!(
        trusted.entries[0].source,
        mcp::config::ServerSource::Project
    );
}

// Verifies: gh #53 - the verbs: status names the server, disable
// persists to the user file, exposure cycles, reconnect reports.
#[test]
fn verbs_act_and_persist() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let dir = root("verbs");
    write(
        &dir.join("data/mcp.json"),
        &format!(
            r#"{{"mcpServers": {{"echo": {{"command": "python3", "args": ["{f}"]}}}}}}"#,
            f = fixture()
        ),
    );
    let loaded = load_entries(&dir.join("data"), &dir.join("project"), false);
    let manager = McpManager::load(
        engine("verbs"),
        dir.join("data/mcp.json"),
        None,
        loaded.entries,
    );
    let status = manager.act("", "");
    assert!(status.contains("echo"), "{status}");
    assert!(status.contains("connected"), "{status}");

    assert_eq!(manager.act("disable", "echo"), "`echo` disabled");
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("data/mcp.json")).expect("read"))
            .expect("json");
    assert_eq!(file["mcpServers"]["echo"]["enabled"], false);
    assert!(manager.act("", "").contains("disabled"));

    assert_eq!(manager.act("enable", "echo"), "`echo` enabled");
    assert!(manager.act("exposure", "echo").contains("deferred"));
    assert!(
        manager
            .act("exposure", "echo deferred")
            .contains("deferred")
    );
    let reconnected = manager.act("reconnect", "echo");
    assert!(reconnected.contains("reconnected"), "{reconnected}");
    assert!(
        manager.act("frobnicate", "echo").contains("unknown"),
        "unknown verbs name themselves"
    );
}

// Verifies: gh #53 - a user-level server edited under a trusted
// project writes a knob-only override (the user file keeps the
// transport), and later edits stay in the override.
#[test]
fn trusted_edits_write_project_overrides() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let dir = root("override");
    write(
        &dir.join("data/mcp.json"),
        &format!(
            r#"{{"mcpServers": {{"echo": {{"command": "python3", "args": ["{f}"]}}}}}}"#,
            f = fixture()
        ),
    );
    let project = dir.join("project");
    let loaded = load_entries(&dir.join("data"), &project, true);
    let manager = McpManager::load(
        engine("override"),
        dir.join("data/mcp.json"),
        Some(project.join(".lca/mcp.json")),
        loaded.entries,
    );
    assert_eq!(manager.act("disable", "echo"), "`echo` disabled");
    let user: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("data/mcp.json")).expect("read"))
            .expect("json");
    assert!(
        user["mcpServers"]["echo"].get("enabled").is_none(),
        "the user file keeps no knob: {user}"
    );
    let project_file: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(project.join(".lca/mcp.json")).expect("read"),
    )
    .expect("json");
    assert_eq!(project_file["mcpServers"]["echo"]["enabled"], false);
    assert!(
        project_file["mcpServers"]["echo"].get("command").is_none(),
        "knobs only: {project_file}"
    );
}

// Verifies: gh #53 - planning under a trusted project without a file
// still hands the manager an override target (creation, not just
// edits).
#[test]
fn plan_points_at_a_missing_project_file() {
    let dir = root("plan");
    write(
        &dir.join("data/mcp.json"),
        r#"{"mcpServers": {"echo": {"command": "true"}}}"#,
    );
    let project = dir.join("project");
    let (_entries, _grants, _warnings, _user, project_path) =
        lca_cli::mcp::McpManager::plan(&dir.join("data"), &project, true);
    assert_eq!(project_path, Some(project.join(".lca/mcp.json")));
    let (_entries, _grants, _warnings, _user, untrusted) =
        lca_cli::mcp::McpManager::plan(&dir.join("data"), &project, false);
    assert_eq!(untrusted, None);
}

// Verifies: gh #53 - the prompt section lists reachable servers only
// (codemode/deferred, enabled), with their reachability line.
#[test]
fn prompt_section_lists_reachable_servers() {
    let dir = root("section");
    write(
        &dir.join("data/mcp.json"),
        r#"{"mcpServers": {
            "echo": {"command": "true", "exposure": "deferred", "description": "Echo."},
            "loud": {"command": "true", "exposure": "direct"},
            "off": {"command": "true", "exposure": "codemode", "enabled": false}
        }}"#,
    );
    let loaded = load_entries(&dir.join("data"), &dir.join("project"), false);
    let section = servers_section(&loaded.entries).expect("section");
    assert!(
        section.contains("- echo: Echo. (load with tool_search, then call directly)."),
        "{section}"
    );
    assert!(!section.contains("loud"), "direct tools declare themselves");
    assert!(!section.contains("off"), "disabled stays silent");
}
