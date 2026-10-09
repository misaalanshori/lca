//! gh #53 phase 3: resources end to end - list, templates, and read
//! through the bridge on both transports, at the widest exposure.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

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

fn stdio_entry(exposure: McpExposure) -> ServerEntry {
    ServerEntry {
        name: "echo".to_string(),
        kind: EntryKind::Stdio {
            command: "python3".to_string(),
            args: vec![fixture()],
            cwd_scope: "workspace".to_string(),
        },
        enabled: true,
        exposure,
        tool_exposure: Vec::new(),
        description: String::new(),
        source: ServerSource::User,
    }
}

fn call(
    bridge: &mcp::McpBridge,
    tool: &str,
    arguments: &str,
) -> Result<lca_protocol::ToolResult, lca_protocol::DispatchError> {
    runtime().block_on(bridge.execute_tool(&lca_protocol::ToolCall {
        call_id: "call-1".to_string(),
        name: tool.to_string(),
        arguments: arguments.to_string(),
        parent_call_id: None,
    }))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
}

// Verifies: gh #53 - the fixture's resources list, template, and
// read through the bridge (stdio): text arrives as text.
#[test]
fn stdio_resources_list_and_read() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let caps = sandbox("resources-stdio", mcp::manifest_grants());
    let bridge = mcp::McpBridge::connect_managed(caps, vec![stdio_entry(McpExposure::Direct)]);
    let names: Vec<String> = bridge
        .tool_specs()
        .expect("specs")
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    for tool in [
        mcp::LIST_RESOURCES_TOOL,
        mcp::LIST_TEMPLATES_TOOL,
        mcp::READ_RESOURCE_TOOL,
    ] {
        assert!(names.contains(&tool.to_string()), "{names:?}");
    }
    let list = call(&bridge, mcp::LIST_RESOURCES_TOOL, "{}").expect("list");
    assert!(list.content.contains("echo://greeting"), "{}", list.content);
    let templates = call(&bridge, mcp::LIST_TEMPLATES_TOOL, "{}").expect("templates");
    assert!(
        templates.content.contains("echo://{name}"),
        "{}",
        templates.content
    );
    let read = call(
        &bridge,
        mcp::READ_RESOURCE_TOOL,
        r#"{"server":"echo","uri":"echo://greeting"}"#,
    )
    .expect("read");
    assert_eq!(read.content, "hello, resource");
    // Binary content stages to a file whose path the model receives.
    let staged = call(
        &bridge,
        mcp::READ_RESOURCE_TOOL,
        r#"{"server":"echo","uri":"echo://bytes"}"#,
    )
    .expect("read bytes");
    let bytes = std::fs::read(staged.content.trim()).expect("staged file reads");
    assert_eq!(bytes, b"binary-bytes");
}

// Verifies: gh #53 - the same trio over HTTP, plus the widest rule:
// a deferred server's resource tools stay deferred.
#[test]
fn http_resources_list_and_read_deferred() {
    let mock = mock_server();
    let caps = sandbox("resources-http", mcp::manifest_grants());
    let entry = ServerEntry {
        name: "remote".to_string(),
        kind: EntryKind::Http {
            url: format!("{}/mcp", mock.base),
            headers: Default::default(),
            timeout_secs: 10,
            oauth: None,
        },
        enabled: true,
        exposure: McpExposure::Deferred,
        tool_exposure: Vec::new(),
        description: String::new(),
        source: ServerSource::User,
    };
    let bridge = mcp::McpBridge::connect_managed(caps, vec![entry]);
    let specs = bridge.tool_specs().expect("specs");
    let resource = specs
        .iter()
        .find(|spec| spec.name == mcp::READ_RESOURCE_TOOL)
        .expect("resource tools register");
    assert_eq!(resource.exposure, lca_protocol::ToolExposure::Deferred);
    let list = call(&bridge, mcp::LIST_RESOURCES_TOOL, "{}").expect("list");
    assert!(list.content.contains("mock://greeting"), "{}", list.content);
    let read = call(
        &bridge,
        mcp::READ_RESOURCE_TOOL,
        r#"{"server":"remote","uri":"mock://greeting"}"#,
    )
    .expect("read");
    assert_eq!(read.content, "hello, remote");
}

// Verifies: gh #53 - no resource capability, no resource tools:
// the plain endpoint lists tools only.
#[test]
fn servers_without_resources_register_none() {
    let mock = mock_server();
    let caps = sandbox("resources-absent", mcp::manifest_grants());
    let bridge = mcp::McpBridge::connect_http(
        caps,
        vec![mcp::HttpServerConfig {
            name: "remote".to_string(),
            url: format!("{}/mcp-plain", mock.base),
            headers: Default::default(),
            timeout_secs: 10,
            oauth: None,
        }],
    )
    .expect("connect");
    let names: Vec<String> = bridge
        .tool_specs()
        .expect("specs")
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(names.contains(&"mcp__remote__echo".to_string()));
    assert!(
        !names.contains(&mcp::READ_RESOURCE_TOOL.to_string()),
        "{names:?}"
    );
}
