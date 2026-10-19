//! GitHub #291: `round_trip` and `Session::resource_templates` read
//! exactly ONE line after sending a request. MCP servers emit JSON-RPC
//! notifications as ordinary traffic (`notifications/message`,
//! `notifications/progress`, `notifications/tools/list_changed`), and a
//! notification carries `method` with no `id`. So the notification was
//! parsed as the reply — `round_trip` failed with "answered without a
//! result", `resource_templates` with "answered another call" — and the
//! *real* reply stayed queued in `buf`, so every later call on that
//! connection read a stale line: a permanent desync that broke every
//! call after the first one to a notification-emitting server.
//!
//! The fix is one selector in the wire kit (`lca_mcp::wire::response_for`,
//! per ADR-0045's "selectors in the wire kit, transports in the
//! extension"): loop until the reply carrying OUR id, discard
//! notifications, fail loud and name a foreign id (a single-outstanding
//! transport cannot legitimately see one).
//!
//! Verifies: end to end through the real managed bridge and a real child
//! process (`fixtures/noisy-mcp-server.py`, which precedes every reply
//! with three notifications of different kinds) — sequential tool calls
//! all succeed with their own results, and the ids stay matched.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_mcp::config::{EntryKind, McpExposure, ServerEntry, ServerSource};
use std::sync::{Arc, Mutex};

/// A grant engine whose roots live in a scratch dir (the MCP tests'
/// `common::sandbox`, inlined here because the regressions crate has no
/// shared `common` module).
fn sandbox(name: &str, mut grants: lca_tools::CapabilityGrants) -> Arc<lca_tools::Capabilities> {
    use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};

    struct Always;
    impl PermissionPrompt for Always {
        fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
            Decision::Always
        }
        fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
            false
        }
    }

    let root = lca_testkit::scratch_path(&format!("lca-gh291-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["project", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("mkdir");
    }
    let roots = ScopeRoots {
        workspace: root.join("project"),
        private: root.join("private"),
        home_config: root.join("config"),
        temp: root.join("tmp"),
        state_dir: root.join("data"),
    };
    grants
        .adhoc_net
        .push(lca_permissions::parse_net_pattern("127.0.0.1").expect("loopback pattern"));
    grants.oauth = Some(lca_permissions::OAuthSettings {
        redirect_path: "/callback".to_string(),
        timeout_seconds: 30,
    });
    grants.credentials = true;
    Arc::new(lca_tools::Capabilities::new(
        "mcp",
        grants,
        roots,
        Arc::new(Mutex::new(Always)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    ))
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn fixture(name: &str) -> String {
    // The regressions crate's manifest dir is the repo root, so the
    // subsystem's fixture is reached by path from there (gh #418 Phase A:
    // the crate moved from `extensions/mcp` to `crates/lca-mcp`).
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/lca-mcp/fixtures")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn noisy_entry(name: &str) -> ServerEntry {
    ServerEntry {
        name: name.to_string(),
        kind: EntryKind::Stdio {
            command: "python3".to_string(),
            args: vec![fixture("noisy-mcp-server.py")],
            cwd_scope: "workspace".to_string(),
        },
        enabled: true,
        exposure: McpExposure::Direct,
        tool_exposure: Vec::new(),
        description: "A server that talks constantly.".to_string(),
        source: ServerSource::User,
    }
}

fn call(name: &str, arguments: serde_json::Value) -> lca_protocol::ToolCall {
    lca_protocol::ToolCall {
        call_id: "call-1".to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
        parent_call_id: None,
    }
}

/// The result text of one tool call, or a panic naming the failure.
fn text_of(result: lca_protocol::ToolResult) -> String {
    assert_eq!(
        result.status,
        lca_protocol::ToolResultStatus::Ok,
        "the call must succeed: {:?}",
        result.content
    );
    result.content
}

// Verifies: gh #291 acceptance 1 - a server emitting notifications
// before every reply still serves, and N sequential calls stay in sync
// (the desync pin: call 2 must not read call 1's reply).
#[tokio::test]
async fn sequential_calls_to_a_chatty_server_stay_in_sync() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let caps = sandbox("gh291-seq", lca_mcp::manifest_grants());
    let bridge = lca_mcp::McpBridge::connect_managed(caps, vec![noisy_entry("noisy")]);
    let statuses = bridge.statuses();
    assert_eq!(statuses.len(), 1);
    assert!(
        matches!(statuses[0].state, lca_mcp::ServerStateKind::Connected),
        "the handshake survived three notifications: {:?}",
        statuses[0].state
    );

    let specs = bridge.tool_specs().expect("specs");
    assert!(
        specs.iter().any(|spec| spec.name == "mcp__noisy__echo"),
        "the tool listed through the notifications: {:?}",
        specs.iter().map(|spec| &spec.name).collect::<Vec<_>>()
    );

    // Four calls in a row: each one gets its own answer, never a
    // neighbour's (the desync showed up from the second call onward).
    for index in 0..4 {
        let payload = format!("message-{index}");
        let result = bridge
            .execute_tool(&call(
                "mcp__noisy__echo",
                serde_json::json!({ "text": payload }),
            ))
            .await
            .expect("the call answers past the notifications");
        assert_eq!(
            text_of(result),
            format!("noisy echo: {payload}"),
            "call {index} answered with its own result"
        );
    }
}

// Verifies: gh #291 acceptance 2 - `resource_templates` (the second
// reader that had the same bug) tolerates notifications too, through
// the real bridge's resource tool.
#[tokio::test]
async fn resource_listing_tolerates_notifications() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let caps = sandbox("gh291-templates", lca_mcp::manifest_grants());
    let bridge = lca_mcp::McpBridge::connect_managed(caps, vec![noisy_entry("noisy")]);
    let specs = bridge.tool_specs().expect("specs");
    assert!(
        specs
            .iter()
            .any(|spec| spec.name == lca_mcp::LIST_TEMPLATES_TOOL),
        "the template tool exists for a server offering resources"
    );

    let result = bridge
        .execute_tool(&call(lca_mcp::LIST_TEMPLATES_TOOL, serde_json::json!({})))
        .await
        .expect("the listing answers past the notifications");
    let text = text_of(result);
    assert!(
        text.contains("noisy://"),
        "the template listing came through: {text}"
    );
}

// Verifies: gh #291 acceptance 3 - a foreign id still fails loud rather
// than being silently consumed (the single-outstanding-request rule),
// pinned at the selector seam the transports share.
#[test]
fn a_foreign_id_is_refused_by_the_shared_selector() {
    let mut lines = vec![
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/progress"}).to_string(),
        serde_json::json!({"jsonrpc": "2.0", "id": 4242, "result": {"stale": true}}).to_string(),
    ]
    .into_iter();
    let err = lca_mcp::wire::response_for(7, || Ok(lines.next()))
        .expect_err("a foreign id is a desync, not noise");
    assert!(err.contains("4242"), "the foreign id is named: {err}");
    assert!(err.contains("desynchronized"), "{err}");
}

// Verifies: gh #291 acceptance 4 - notification-only traffic never
// returns success: the reader keeps waiting and ends on EOF, the same
// failure shape a closed stdout has always had.
#[test]
fn notification_only_traffic_never_reports_success() {
    let mut lines = vec![
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/message"}).to_string(),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
            .to_string(),
    ]
    .into_iter();
    let err =
        lca_mcp::wire::response_for(1, || Ok(lines.next())).expect_err("no reply means no success");
    assert!(err.contains("closed its stdout"), "{err}");
}
