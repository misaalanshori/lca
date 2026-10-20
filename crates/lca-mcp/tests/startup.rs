//! gh #418 Phase E: MCP startup must not block the interface (NFR-3).
//!
//! The defect was structural: `McpManager::load` connected every configured
//! server synchronously, so cold start was the sum of every server's timeout
//! — a hung or unreachable server could hold the TUI shut for a minute.
//!
//! These rows use a fixture whose `LCA_PROBE_DELAY` sleeps before answering
//! `initialize`, so "did this block?" is a wall-clock question with a large
//! margin rather than a scheduling coincidence.

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

fn fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/spawn-probe-mcp-server.py")
        .to_string_lossy()
        .replace('\\', "/")
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn root(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(&format!("lca-mcp-e-{name}"));
    for part in ["workspace", "private", "config", "data", "tmp"] {
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

/// One entry naming the fixture, with a handshake delay of `delay` seconds.
fn slow_entry(delay: u64, exposure: McpExposure) -> ServerEntry {
    let text = format!(
        r#"{{"mcpServers": {{"slow": {{
            "command": "python3",
            "args": ["{}"],
            "env": {{"LCA_PROBE_DELAY": "{delay}"}},
            "exposure": "{}"
        }}}}}}"#,
        fixture(),
        match exposure {
            McpExposure::Direct => "direct",
            McpExposure::Codemode => "codemode",
            McpExposure::Deferred => "deferred",
            McpExposure::Hidden => "hidden",
        }
    );
    let parsed = parse_mcp_json(&text, ServerSource::User);
    assert_eq!(parsed.warnings, Vec::<String>::new(), "no warnings");
    parsed.servers.into_iter().next().expect("one entry")
}

// Verifies: gh #418 Phase E (NFR-3) - `McpManager::load` returns immediately
// even when the server will take seconds to handshake. The measurement is the
// row: a 3-second server must not cost 3 seconds of startup.
#[test]
fn loading_a_slow_server_returns_without_waiting_for_it() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let dir = root("slow-load");
    let started = std::time::Instant::now();
    let manager = lca_mcp::McpManager::load(
        engine(&dir),
        dir.join("data/mcp.json"),
        None,
        vec![slow_entry(3, McpExposure::Direct)],
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(1500),
        "load returns without waiting for a 3s handshake, took {elapsed:?}"
    );
    // And the row says so honestly instead of claiming a connection.
    assert_eq!(
        manager.bridge().connecting_names(),
        vec!["slow".to_string()],
        "the server is reported as connecting, not silently absent"
    );
    assert!(
        manager.status_text().contains("connecting"),
        "`/mcp` says connecting: {}",
        manager.status_text()
    );
}

// Verifies: gh #418 Phase E - the first turn waits (bounded) for a `direct`
// server and then sees its tool, so the first request can declare it. This is
// the half that makes non-blocking startup safe: nothing is lost, only
// deferred.
#[test]
fn the_first_turn_wait_sees_a_direct_servers_tools() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let dir = root("first-turn");
    let manager = lca_mcp::McpManager::load(
        engine(&dir),
        dir.join("data/mcp.json"),
        None,
        vec![slow_entry(1, McpExposure::Direct)],
    );
    assert_eq!(
        manager.bridge().tool_specs().expect("specs").len(),
        0,
        "nothing is declared before the wait"
    );
    let still = manager.wait_for_startup_with(std::time::Duration::from_secs(10));
    assert!(
        still.is_empty(),
        "the server landed inside the budget: {still:?}"
    );
    let specs = manager.bridge().tool_specs().expect("specs");
    assert!(
        specs.iter().any(|spec| spec.name == "mcp__slow__probe"),
        "the tool is declared after the wait: {:?}",
        specs.iter().map(|spec| &spec.name).collect::<Vec<_>>()
    );
}

// Verifies: gh #418 Phase E - the wait is bounded. A server slower than the
// budget does not hold the turn: the wait returns the names still connecting,
// which is what the host reports, and the turn proceeds without that tool.
#[test]
fn the_startup_wait_gives_up_at_its_budget() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let dir = root("budget");
    let manager = lca_mcp::McpManager::load(
        engine(&dir),
        dir.join("data/mcp.json"),
        None,
        vec![slow_entry(5, McpExposure::Direct)],
    );
    let started = std::time::Instant::now();
    let still = manager.wait_for_startup_with(std::time::Duration::from_millis(300));
    let elapsed = started.elapsed();
    assert_eq!(
        still,
        vec!["slow".to_string()],
        "the still-connecting server is named"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "the budget bounds the wait even when the server needs longer: {elapsed:?}"
    );
}

// Verifies: gh #418 Phase E (pi's rule, `index.ts` `waitForDirectServers`) -
// only servers whose tools are **declared** to the model justify holding a
// turn. A `codemode`/`deferred` server is reached through discovery, which
// waits on demand, so the first turn does not wait for it at all.
#[test]
fn a_non_direct_server_does_not_hold_the_first_turn() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let dir = root("non-direct");
    let manager = lca_mcp::McpManager::load(
        engine(&dir),
        dir.join("data/mcp.json"),
        None,
        vec![slow_entry(5, McpExposure::Deferred)],
    );
    let started = std::time::Instant::now();
    let still = manager.wait_for_startup_with(std::time::Duration::from_secs(10));
    let elapsed = started.elapsed();
    assert!(still.is_empty(), "no direct server to wait for: {still:?}");
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "a deferred server never holds the turn: {elapsed:?}"
    );
}

// Verifies: gh #418 Phase E - a disabled entry is never spawned by the
// background path either. The disabled knob is honoured at `spawn_connections`,
// so a disabled server costs no process and lands its row immediately.
#[test]
fn a_disabled_entry_neither_spawns_nor_waits() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let dir = root("disabled-slow");
    let mut entry = slow_entry(5, McpExposure::Direct);
    entry.enabled = false;
    let started = std::time::Instant::now();
    let manager =
        lca_mcp::McpManager::load(engine(&dir), dir.join("data/mcp.json"), None, vec![entry]);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "a disabled entry costs nothing"
    );
    assert!(
        manager.bridge().connecting_names().is_empty(),
        "nothing is connecting"
    );
    assert!(
        manager.status_text().contains("disabled"),
        "/mcp says disabled: {}",
        manager.status_text()
    );
}
