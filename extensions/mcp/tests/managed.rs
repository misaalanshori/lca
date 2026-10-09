//! gh #53 phase 3: the managed bridge - mixed entries land per-server
//! states, exposures ride the specs, and deferred tools surface
//! through the real `tool_search` table.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::Arc;

mod common;

use common::{mock_server, sandbox};
use mcp::config::{EntryKind, McpExposure, ServerEntry, ServerSource};

fn fixture() -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/echo-mcp-server.py")
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

fn stdio_entry(name: &str, exposure: McpExposure, enabled: bool) -> ServerEntry {
    ServerEntry {
        name: name.to_string(),
        kind: EntryKind::Stdio {
            command: "python3".to_string(),
            args: vec![fixture()],
            cwd_scope: "workspace".to_string(),
        },
        enabled,
        exposure,
        tool_exposure: Vec::new(),
        description: format!("{name} server."),
        source: ServerSource::User,
    }
}

// Verifies: gh #53 - one failing server never blocks the rest: the
// connected tool serves, and every entry lands its state row.
#[test]
fn one_failure_never_blocks_the_rest() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let caps = sandbox("managed-mixed", mcp::manifest_grants());
    let mut failing = stdio_entry("failing", McpExposure::Direct, true);
    failing.kind = EntryKind::Stdio {
        command: "mcp-p53-definitely-missing-binary".to_string(),
        args: Vec::new(),
        cwd_scope: "workspace".to_string(),
    };
    let bridge = mcp::McpBridge::connect_managed(
        caps,
        vec![
            stdio_entry("echo", McpExposure::Deferred, true),
            stdio_entry("off", McpExposure::Direct, false),
            failing,
        ],
    );
    let statuses = bridge.statuses();
    assert_eq!(statuses.len(), 3);
    let state = |name: &str| {
        statuses
            .iter()
            .find(|status| status.name == name)
            .expect("row")
            .state
            .clone()
    };
    assert!(matches!(state("echo"), mcp::ServerStateKind::Connected));
    assert!(matches!(state("off"), mcp::ServerStateKind::Disabled));
    assert!(matches!(state("failing"), mcp::ServerStateKind::Failed(_)));
    // Only the connected server serves, under its own exposure.
    let specs = bridge.tool_specs().expect("specs");
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "mcp__echo__echo");
    assert_eq!(specs[0].exposure, lca_protocol::ToolExposure::Deferred);
}

// Verifies: gh #53 - a guarded remote server lands needs-sign-in
// (with the scopes named), not failure.
#[test]
fn guarded_remote_lands_needs_sign_in() {
    let mock = mock_server();
    mock.guard(Some("mock-access"), Some(r#"Bearer scope="extra""#));
    let caps = sandbox("managed-auth", mcp::manifest_grants());
    let entry = mcp::config::ServerEntry {
        name: "remote".to_string(),
        kind: mcp::config::EntryKind::Http {
            url: format!("{}/mcp", mock.base),
            headers: Default::default(),
            timeout_secs: 10,
            oauth: None,
        },
        enabled: true,
        exposure: McpExposure::Direct,
        tool_exposure: Vec::new(),
        description: String::new(),
        source: ServerSource::User,
    };
    let bridge = mcp::McpBridge::connect_managed(caps, vec![entry]);
    let statuses = bridge.statuses();
    assert_eq!(statuses.len(), 1);
    match &statuses[0].state {
        mcp::ServerStateKind::NeedsSignIn(detail) => {
            assert!(detail.contains("extra"), "{detail}")
        }
        other => panic!("needs sign-in, not {other:?}"),
    }
    assert!(bridge.tool_specs().expect("specs").is_empty());
}

// Verifies: gh #53 - deferred MCP tools surface through the real
// discovery table (`tool_search` finds them; activation declares
// them), the #77 interplay shape.
#[test]
fn deferred_tools_surface_through_tool_search() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let caps = sandbox("managed-search", mcp::manifest_grants());
    let bridge = mcp::McpBridge::connect_managed(
        caps,
        vec![stdio_entry("echo", McpExposure::Deferred, true)],
    );
    let mut registry = lca_core::ExtensionRegistry::new();
    registry.register(Arc::new(bridge));
    let hits = registry.tool_search("echo");
    assert!(
        hits.iter().any(|spec| spec.name == "mcp__echo__echo"),
        "deferred tools are discoverable: {:?}",
        hits.iter().map(|spec| &spec.name).collect::<Vec<_>>()
    );
    assert!(
        registry
            .declared_tool_specs()
            .iter()
            .all(|spec| spec.name != "mcp__echo__echo"),
        "but never declared until activated"
    );
}
