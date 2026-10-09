//! gh #53 phase 1: a scripted turn end to end through the MCP bridge -
//! the model calls `mcp__echo__echo`, the echo fixture answers, and the
//! server's answer is the tool result the turn records.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_core::{Agent, AgentConfig, ExtensionRegistry, TurnEvent, TurnSink, TurnStatus};
use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_protocol::ToolResultStatus;
use lca_testkit::{FakeProvider, fake_usage};

fn scratch(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(name);
    for part in ["project", "workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(dir.join(part)).expect("mkdir");
    }
    dir
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

struct Allow;

impl PermissionPrompt for Allow {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

#[derive(Default)]
struct CollectingSink {
    events: Vec<TurnEvent>,
}

impl TurnSink for CollectingSink {
    fn on_event(&mut self, event: TurnEvent) {
        self.events.push(event);
    }
}

// Verifies: gh #53 - the fixture echo turn: a tool call to the bridged
// server resolves through the extension dispatch and the server's
// answer lands on the turn's tool result.
#[tokio::test]
async fn an_echo_turn_round_trips_through_the_bridge() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = scratch("mcp-turn");
    let project = root.join("project");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../extensions/mcp/fixtures/echo-mcp-server.py");

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
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("bridge-grants.json")).expect("grants"),
        )),
        root.join("workspace"),
        None,
    ));
    let bridge = mcp::McpBridge::connect(
        caps,
        vec![mcp::ServerConfig {
            name: "echo".to_string(),
            command: "python3".to_string(),
            args: vec![fixture.to_string_lossy().into_owned()],
            cwd_scope: "workspace".to_string(),
        }],
    )
    .expect("the fixture server connects");
    let mut registry = ExtensionRegistry::new();
    registry.register(Arc::new(bridge));

    let provider = FakeProvider::builder()
        .turn(|t| {
            t.tool_call("mcp__echo__echo", r#"{"text":"hello"}"#)
                .usage(fake_usage(10, 10, 0, 10))
        })
        .turn(|t| t.text("done").usage(fake_usage(20, 5, 10, 0)))
        .build();

    let store = lca_session::SessionStore::new(root.join("data"));
    let session = store.create_session(&project, "test").expect("session");
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&root.join("grants.json")).expect("grants"),
    ));
    let mut tools = lca_tools::ToolExecutor::new(
        Arc::new(lca_tools::NativeOps::default()),
        project.clone(),
        project.clone(),
        65536,
        Some(std::time::Duration::from_secs(30)),
    );
    let mut config = AgentConfig {
        provider: "fake".to_string(),
        model: "faux-1".to_string(),
        retry_base_delay: std::time::Duration::ZERO,
        ..AgentConfig::default()
    };
    config.extensions = Arc::new(registry);
    let mut prompt = Allow;
    let mut sink = CollectingSink::default();
    let outcome = {
        let mut agent = Agent::new(
            &store,
            &session,
            &provider,
            &mut tools,
            grants.clone(),
            &mut prompt,
            None,
            config.clone(),
        );
        agent
            .run_turn("say it back", &mut sink, &lca_tools::CancelFlag::new())
            .await
    };
    assert_eq!(outcome.status, TurnStatus::Ok);
    let results: Vec<_> = sink
        .events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::ToolFinished(result) => Some(result.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, ToolResultStatus::Ok);
    assert_eq!(results[0].content, "echo: hello");
}
