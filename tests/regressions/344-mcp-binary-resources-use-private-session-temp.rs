//! GitHub #344: binary MCP resources must be staged in the active session's
//! private temp scope, not in a shared predictable directory.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::{Arc, Mutex};

use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use mcp::config::{EntryKind, McpExposure, ServerEntry, ServerSource};

struct Always;

impl PermissionPrompt for Always {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }

    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn call(bridge: &mcp::McpBridge) -> lca_protocol::ToolResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(bridge.execute_tool(&lca_protocol::ToolCall {
            call_id: "binary-resource".to_string(),
            name: mcp::READ_RESOURCE_TOOL.to_string(),
            arguments: r#"{"server":"echo","uri":"echo://bytes"}"#.to_string(),
            parent_call_id: None,
        }))
        .expect("read MCP resource")
}

#[test]
fn binary_resources_are_staged_in_private_session_temp() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }

    let root = lca_testkit::scratch_path("lca-gh344-mcp-resource");
    let _ = std::fs::remove_dir_all(&root);
    let workspace = root.join("project");
    let data = root.join(".lca");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let temp = lca_cli::ensure_session_temp(&data, "session-344").expect("session temp");
    let grants = mcp::manifest_grants();
    let caps = Arc::new(lca_tools::Capabilities::new(
        "mcp",
        grants,
        ScopeRoots {
            workspace: workspace.clone(),
            private: data.join("private"),
            home_config: root.join("config"),
            temp: temp.clone(),
            state_dir: data.clone(),
        },
        Arc::new(Mutex::new(Always)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        workspace,
        None,
    ));
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("extensions/mcp/fixtures/echo-mcp-server.py")
        .to_string_lossy()
        .into_owned();
    let bridge = mcp::McpBridge::connect_managed(
        caps,
        vec![ServerEntry {
            name: "echo".to_string(),
            kind: EntryKind::Stdio {
                command: "python3".to_string(),
                args: vec![fixture],
                cwd_scope: "workspace".to_string(),
            },
            enabled: true,
            exposure: McpExposure::Direct,
            tool_exposure: Vec::new(),
            description: String::new(),
            source: ServerSource::User,
        }],
    );

    let result = call(&bridge);
    let staged = std::path::PathBuf::from(result.content.trim());
    let canonical_temp = std::fs::canonicalize(&temp).expect("canonical temp");
    assert!(staged.is_absolute(), "the model receives an absolute path");
    assert!(
        staged.starts_with(&canonical_temp),
        "binary resource escaped session temp: {staged:?}"
    );
    assert_eq!(
        std::fs::read(&staged).expect("read staged file"),
        b"binary-bytes"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&staged)
                .expect("staged metadata")
                .permissions()
                .mode()
                & 0o077,
            0
        );
        assert_eq!(
            std::fs::metadata(&temp)
                .expect("temp metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    drop(bridge);
    let _ = std::fs::remove_dir_all(root);
}
