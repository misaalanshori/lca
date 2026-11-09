//! gh #53 phase 3: the managed bridge - mixed entries land per-server
//! states, exposures ride the specs, and deferred tools surface
//! through the real `tool_search` table.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod common;

use common::{mock_server, sandbox};
use lca_mcp::config::{EntryKind, McpExposure, ServerEntry, ServerSource};

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
            cwd: None,
            env: std::collections::BTreeMap::new(),
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
    let caps = sandbox("managed-mixed", lca_mcp::manifest_grants());
    let mut failing = stdio_entry("failing", McpExposure::Direct, true);
    failing.kind = EntryKind::Stdio {
        command: "mcp-p53-definitely-missing-binary".to_string(),
        args: Vec::new(),
        cwd: None,
        env: std::collections::BTreeMap::new(),
    };
    let bridge = lca_mcp::McpBridge::connect_managed(
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
    assert!(matches!(state("echo"), lca_mcp::ServerStateKind::Connected));
    assert!(matches!(state("off"), lca_mcp::ServerStateKind::Disabled));
    assert!(matches!(
        state("failing"),
        lca_mcp::ServerStateKind::Failed(_)
    ));
    // Only the connected server serves, under its own exposure:
    // the echo tool plus the resource trio (the fixture offers
    // resources), all deferred.
    let specs = bridge.tool_specs().expect("specs");
    assert_eq!(specs.len(), 4);
    for spec in &specs {
        assert_eq!(spec.exposure, lca_protocol::ToolExposure::Deferred);
    }
    assert!(specs.iter().any(|spec| spec.name == "mcp__echo__echo"));
    assert!(
        specs
            .iter()
            .any(|spec| spec.name == lca_mcp::READ_RESOURCE_TOOL)
    );
}

// Verifies: gh #585 - a state row carries the handshake's resource answer, and a row
// with no connection carries nothing (which reads as "not asked").
//
// The CLI row (`crates/lca-cli/tests/mcp_cli.rs`) drives the printed report; this is
// the fact underneath it, at the one place a live fact becomes a reported one. A
// disabled or failed entry has no `live`, so `false` there is honest silence rather
// than a claim - which is exactly the reading `docs/mcp.md` states.
#[test]
fn a_status_row_reports_the_handshakes_resource_answer() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let caps = sandbox("managed-resources-flag", lca_mcp::manifest_grants());
    let mut failing = stdio_entry("failing", McpExposure::Direct, true);
    failing.kind = EntryKind::Stdio {
        command: "mcp-585-definitely-missing-binary".to_string(),
        args: Vec::new(),
        cwd: None,
        env: std::collections::BTreeMap::new(),
    };
    let bridge = lca_mcp::McpBridge::connect_managed(
        caps,
        vec![
            stdio_entry("echo", McpExposure::Direct, true),
            stdio_entry("off", McpExposure::Direct, false),
            failing,
        ],
    );
    let flag = |name: &str| {
        bridge
            .statuses()
            .into_iter()
            .find(|status| status.name == name)
            .unwrap_or_else(|| panic!("{name} has a row"))
            .offers_resources
    };
    assert!(
        flag("echo"),
        "the fixture advertises `capabilities.resources`, so the connected row says so"
    );
    assert!(!flag("off"), "a disabled server was never asked");
    assert!(!flag("failing"), "a failed server was never asked");
}

// Verifies: gh #53 - a guarded remote server lands needs-sign-in
// (with the scopes named), not failure.
#[test]
fn guarded_remote_lands_needs_sign_in() {
    let mock = mock_server();
    mock.guard(Some("mock-access"), Some(r#"Bearer scope="extra""#));
    let caps = sandbox("managed-auth", lca_mcp::manifest_grants());
    let entry = lca_mcp::config::ServerEntry {
        name: "remote".to_string(),
        kind: lca_mcp::config::EntryKind::Http {
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
    let bridge = lca_mcp::McpBridge::connect_managed(caps, vec![entry]);
    let statuses = bridge.statuses();
    assert_eq!(statuses.len(), 1);
    match &statuses[0].state {
        lca_mcp::ServerStateKind::NeedsSignIn(detail) => {
            assert!(detail.contains("extra"), "{detail}")
        }
        other => panic!("needs sign-in, not {other:?}"),
    }
    assert!(bridge.tool_specs().expect("specs").is_empty());
}

// Verifies: gh #586 - the state row counts the server's **listing**, and the
// inspector shows the hidden tool instead of pretending the server is empty.
//
// The defect: `status.tools` counted `live.specs()`, which drops `hidden` tools,
// so a hidden server read `connected, 0 tools` while `pi 1.3.0` read
// `connected, 1 tool` for the same config - and `/mcp`'s tool inspector, reading
// the same filtered set, showed nothing at all. A hidden tool is not offered to
// the model; the server still has it.
#[test]
fn a_hidden_server_counts_and_inspects_its_listing() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let caps = sandbox("managed-hidden-listing", lca_mcp::manifest_grants());
    let bridge = lca_mcp::McpBridge::connect_managed(
        caps,
        vec![stdio_entry("quiet", McpExposure::Hidden, true)],
    );
    let status = bridge
        .statuses()
        .into_iter()
        .find(|status| status.name == "quiet")
        .expect("the row landed");
    assert!(
        matches!(status.state, lca_mcp::ServerStateKind::Connected),
        "the server connected: {:?}",
        status.state
    );
    assert_eq!(
        status.tools, 1,
        "the listing has the echo tool; a hidden exposure does not remove it from the server"
    );
    // The inspector answers from the same listing, so the two agree - and the
    // annotation it renders is the effective exposure, `hidden`.
    let inspected = bridge.server_tools("quiet");
    assert_eq!(
        inspected.len(),
        1,
        "the inspector shows the hidden tool: {inspected:?}"
    );
    assert_eq!(
        inspected[0].exposure,
        lca_protocol::ToolExposure::Hidden,
        "and marks it hidden"
    );
    // What the *model* can reach is still nothing: the two views differ by
    // exactly the hidden entries.
    assert!(
        bridge
            .tool_specs()
            .expect("specs")
            .iter()
            .all(|spec| !spec.name.starts_with("mcp__quiet__")),
        "a hidden server contributes no callable spec: {:?}",
        bridge
            .tool_specs()
            .expect("specs")
            .iter()
            .map(|s| &s.name)
            .collect::<Vec<_>>()
    );
}

// Verifies: gh #586 - the listing keeps a hidden tool's assigned name and the
// server's default exposure is carried, not invented.
//
// `assigned_pairs` is the unfiltered planner; `qualified_pairs` is it minus the
// hidden entries. Pinning the relationship (rather than each set separately) is
// what keeps a future rule change from applying to one view only.
#[test]
fn the_listing_is_the_unfiltered_plan_and_specs_are_its_hidden_free_subset() {
    let tool = |name: &str| lca_mcp::ServerTool {
        name: name.to_string(),
        qualified: name.to_string(),
        description: String::new(),
        parameters: serde_json::json!({}),
        annotations: lca_protocol::ToolAnnotations::default(),
    };
    use lca_mcp::config::McpExposure;
    let bonus = |pairs: Vec<(String, lca_protocol::ToolSpec)>| -> Vec<(String, String)> {
        pairs
            .into_iter()
            .map(|(original, spec)| (original, spec.exposure.as_str().to_string()))
            .collect()
    };
    let assigned = bonus(
        lca_mcp::assigned_pairs(
            "s",
            vec![tool("a"), tool("gone"), tool("b")],
            McpExposure::Codemode,
            &[("gone".to_string(), McpExposure::Hidden)],
        )
        .expect("planned"),
    );
    assert_eq!(
        assigned,
        vec![
            ("a".to_string(), "codemode".to_string()),
            ("b".to_string(), "codemode".to_string()),
            ("gone".to_string(), "hidden".to_string()),
        ],
        "the listing carries every listed tool with its effective exposure: {assigned:?}"
    );
    let reachable = bonus(
        lca_mcp::qualified_pairs(
            "s",
            vec![tool("a"), tool("gone"), tool("b")],
            McpExposure::Codemode,
            &[("gone".to_string(), McpExposure::Hidden)],
        )
        .expect("planned"),
    );
    assert_eq!(
        reachable,
        vec![
            ("a".to_string(), "codemode".to_string()),
            ("b".to_string(), "codemode".to_string()),
        ],
        "the callable subset drops exactly the hidden one: {reachable:?}"
    );
}
