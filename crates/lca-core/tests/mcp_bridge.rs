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
    // gh #418 Phase A: path-constant update only - the crate moved from
    // `extensions/mcp` to `crates/lca-mcp`; the assertion below is unchanged.
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../lca-mcp/fixtures/echo-mcp-server.py");

    let caps = Arc::new(lca_tools::Capabilities::new(
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
            GrantStore::open(&root.join("bridge-grants.json")).expect("grants"),
        )),
        root.join("workspace"),
        None,
    ));
    let bridge = lca_mcp::McpBridge::connect(
        caps,
        vec![lca_mcp::ServerConfig {
            name: "echo".to_string(),
            command: "python3".to_string(),
            args: vec![fixture.to_string_lossy().into_owned()],
            cwd: None,
            env: std::collections::BTreeMap::new(),
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

// Verifies: gh #53 - deferred MCP tools surface through the real discovery
// table (`tool_search` finds them; activation declares them), the #77
// interplay shape.
//
// gh #418 Phase A moved this row here from `crates/lca-mcp/tests/managed.rs`.
// It drives `lca_core::ExtensionRegistry`, so it belongs on the core's side of
// the boundary: `lca-core` may dev-depend on `lca-mcp`, never the reverse, and
// the architecture guard pins that. Assertions are unchanged - only the crate
// path and the sandbox helper differ.
#[test]
fn deferred_mcp_tools_surface_through_tool_search() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = scratch("mcp-deferred-search");
    let project = root.join("project");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../lca-mcp/fixtures/echo-mcp-server.py");
    let caps = Arc::new(lca_tools::Capabilities::new(
        "mcp",
        lca_mcp::manifest_grants(),
        ScopeRoots {
            workspace: project.clone(),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        )),
        project,
        None,
    ));
    let entry = lca_mcp::config::ServerEntry {
        name: "echo".to_string(),
        kind: lca_mcp::config::EntryKind::Stdio {
            command: "python3".to_string(),
            args: vec![fixture.to_string_lossy().into_owned()],
            cwd: None,
            env: std::collections::BTreeMap::new(),
        },
        enabled: true,
        exposure: lca_mcp::config::McpExposure::Deferred,
        tool_exposure: Vec::new(),
        description: "echo server.".to_string(),
        source: lca_mcp::config::ServerSource::User,
    };
    let bridge = lca_mcp::McpBridge::connect_managed(caps, vec![entry]);
    let mut registry = ExtensionRegistry::new();
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
/// `mcp.json`'s word for one exposure level.
fn exposure_word(exposure: lca_mcp::config::McpExposure) -> &'static str {
    match exposure {
        lca_mcp::config::McpExposure::Direct => "direct",
        lca_mcp::config::McpExposure::Codemode => "codemode",
        lca_mcp::config::McpExposure::Deferred => "deferred",
        lca_mcp::config::McpExposure::Hidden => "hidden",
    }
}

/// One entry naming the probe fixture with a handshake delay, at `exposure`
/// (gh #584): a server that is still `Connecting` when the registry is built,
/// which is what gh #418 Phase E made the ordinary case.
fn probe_entry(
    name: &str,
    delay: u64,
    exposure: lca_mcp::config::McpExposure,
) -> lca_mcp::config::ServerEntry {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../lca-mcp/fixtures/spawn-probe-mcp-server.py")
        .to_string_lossy()
        .replace('\\', "/");
    let mut servers = serde_json::Map::new();
    servers.insert(
        name.to_string(),
        serde_json::json!({
            "command": "python3",
            "args": [fixture],
            "env": { "LCA_PROBE_DELAY": delay.to_string() },
            "exposure": exposure_word(exposure),
        }),
    );
    let text = serde_json::json!({ "mcpServers": servers }).to_string();
    let parsed = lca_mcp::config::parse_mcp_json(&text, lca_mcp::config::ServerSource::User);
    assert!(
        parsed.warnings.is_empty(),
        "no warnings: {:?}",
        parsed.warnings
    );
    parsed.servers.into_iter().next().expect("one entry")
}

/// Run one dispatch future to completion (the rows below are synchronous: they
/// are about the registry's tables, not about an async turn).
fn run_tool(
    future: lca_ext_abi::DispatchFuture<
        '_,
        Result<lca_protocol::ToolResult, lca_protocol::DispatchError>,
    >,
) -> lca_protocol::ToolResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
        .expect("the tool answers")
}

// Verifies: gh #584 - a server that lands **after** `register` is declared to
// the model on the next request **and** callable.
//
// The two halves of the shipped defect, driven through the real bridge and the
// real registry: `declared_tool_specs()` used to omit it (the model was never
// told the tool existed, and `pi 1.3.0` told its model on the first request),
// and `tool_owner` used to answer `None`, so the turn answered
// `unknown tool mcp__probe__probe` while `/mcp` said `connected (1 tools)`.
//
// Registration happens against a table that is still `Connecting`, exactly as
// `registry::assemble` registers the bridge before any background worker
// finishes - which is what gh #418 Phase E made the ordinary case.
#[test]
fn a_server_that_lands_after_registration_is_declared_and_callable() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = scratch("mcp-late-land");
    let project = root.join("project");
    let caps = Arc::new(lca_tools::Capabilities::new(
        "mcp",
        lca_mcp::manifest_grants(),
        ScopeRoots {
            workspace: project.clone(),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        )),
        project,
        None,
    ));
    let manager = lca_mcp::McpManager::load(
        caps,
        root.join("data/mcp.json"),
        None,
        vec![probe_entry(
            "probe",
            3,
            lca_mcp::config::McpExposure::Direct,
        )],
    );
    let mut registry = ExtensionRegistry::new();
    registry.register(manager.bridge());

    assert!(
        registry
            .declared_tool_specs()
            .iter()
            .all(|spec| spec.name != "mcp__probe__probe"),
        "a still-connecting server contributes nothing yet"
    );
    assert!(
        registry.tool_owner("mcp__probe__probe").is_none(),
        "and it has no owner yet either"
    );

    let pending = manager.wait_until_settled(std::time::Duration::from_secs(30));
    assert!(pending.is_empty(), "the slow server lands: {pending:?}");

    let declared: Vec<String> = registry
        .declared_tool_specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(
        declared.iter().any(|name| name == "mcp__probe__probe"),
        "the landed tool is declared on the next request (gh #584): {declared:?}"
    );
    let owner = registry
        .tool_owner("mcp__probe__probe")
        .expect("the landed tool has an owner");
    assert_eq!(owner.name(), "mcp", "and dispatch reaches the bridge");
    assert!(
        registry.is_callable("mcp__probe__probe"),
        "so the tool is callable, not just declared (gh #584)"
    );
    // The turn's own call path validates against the kept spec, so the schema
    // must travel with the late name or validation would see a different spec
    // than the declaration did.
    assert!(
        registry.tool_schema("mcp__probe__probe").is_some(),
        "the schema travelled with the late name"
    );
}

// Verifies: gh #584 - a landed server's tool really runs, through the registry's
// dispatch, not only through the bridge directly.
//
// The declaration half could pass while dispatch still failed; this drives the
// owner's `execute_tool` the way the turn does and asserts the fixture's answer.
#[test]
fn a_late_landed_servers_tool_runs_through_the_registry_dispatch() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = scratch("mcp-late-call");
    let project = root.join("project");
    let caps = Arc::new(lca_tools::Capabilities::new(
        "mcp",
        lca_mcp::manifest_grants(),
        ScopeRoots {
            workspace: project.clone(),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        )),
        project,
        None,
    ));
    let manager = lca_mcp::McpManager::load(
        caps,
        root.join("data/mcp.json"),
        None,
        vec![probe_entry(
            "probe",
            3,
            lca_mcp::config::McpExposure::Direct,
        )],
    );
    let mut registry = ExtensionRegistry::new();
    registry.register(manager.bridge());
    let pending = manager.wait_until_settled(std::time::Duration::from_secs(30));
    assert!(pending.is_empty(), "the slow server lands: {pending:?}");

    let owner = registry
        .tool_owner("mcp__probe__probe")
        .expect("the late tool has an owner")
        .clone();
    let call = lca_protocol::ToolCall {
        call_id: "c1".to_string(),
        name: "mcp__probe__probe".to_string(),
        arguments: "{}".to_string(),
        parent_call_id: None,
    };
    let result = run_tool(owner.execute_tool(&call));
    assert_eq!(
        result.status,
        ToolResultStatus::Ok,
        "the probe answers: {result:?}"
    );
    assert_eq!(
        result.content, "pong",
        "and its answer is the fixture's own (spawn-probe's `probe` tool)"
    );
}

// Verifies: gh #584 (the exposure paths) - a late `hidden` server is neither
// declared nor discoverable, and a late `codemode` one is discoverable through
// `tool_search` without being declared.
//
// Both are the *same* re-plan with a different filter applied afterwards, so this
// row is what proves the re-plan did not quietly widen the exposure model while
// fixing the timing.
#[test]
fn a_late_hidden_server_stays_unreachable_and_a_late_codemode_one_is_discoverable() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = scratch("mcp-late-exposure");
    let project = root.join("project");
    let caps = Arc::new(lca_tools::Capabilities::new(
        "mcp",
        lca_mcp::manifest_grants(),
        ScopeRoots {
            workspace: project.clone(),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        )),
        project,
        None,
    ));
    // Two servers whose tool names collide (`mcp__probe__probe` both ways), so
    // this row also pins that the *planning* rules - not only the exposure
    // filter - ran again: the second loses the name and reports it.
    let manager = lca_mcp::McpManager::load(
        caps,
        root.join("data/mcp.json"),
        None,
        vec![
            probe_entry("probe", 1, lca_mcp::config::McpExposure::Codemode),
            probe_entry("probehidden", 1, lca_mcp::config::McpExposure::Hidden),
        ],
    );
    let mut registry = ExtensionRegistry::new();
    registry.register(manager.bridge());
    let pending = manager.wait_until_settled(std::time::Duration::from_secs(30));
    assert!(pending.is_empty(), "both servers land: {pending:?}");

    let declared: Vec<String> = registry
        .declared_tool_specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(
        declared.iter().all(|name| !name.contains("probe")),
        "neither a codemode nor a hidden tool is declared: {declared:?}"
    );
    let discovered: Vec<String> = registry
        .tool_search("probe")
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert_eq!(
        discovered,
        vec!["mcp__probe__probe".to_string()],
        "only the codemode tool is discoverable (gh #77); the hidden one never is"
    );
    assert!(
        registry.is_callable("mcp__probe__probe"),
        "and the codemode tool is callable once found"
    );
    assert!(
        !registry.is_callable("mcp__probehidden__probe"),
        "a hidden tool is not callable even though it is registered"
    );
}
