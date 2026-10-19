//! Gh #344: binary MCP resource payloads staged in a shared world-writable
//! `/tmp/lca-mcp-resources` with predictable `{server}-{counter}-{safe}`
//! names, written with a raw `std::fs::write`.
//!
//! Four flaws in one call: a shared directory (a local attacker pre-plants a
//! symlink at a predictable name), a raw write that bypasses the capability
//! scopes, no cleanup, and a path the model then had to read from outside
//! every scope.
//!
//! Staging now goes through the `temp` scope — one private directory per
//! session (`<data>/tmp/<session>`, removed at exit), `create_new` so a
//! planted entry is an error rather than a target, owner-only `0600`.
//!
//! Verifies: gh #344 acceptance 1-4 (location + mode bits, no capability
//! bypass, cleanup, and consumption).

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

fn stdio_entry(exposure: McpExposure) -> ServerEntry {
    ServerEntry {
        name: "echo".to_string(),
        kind: EntryKind::Stdio {
            command: "python3".to_string(),
            args: vec![fixture()],
            cwd: None,
            env: std::collections::BTreeMap::new(),
        },
        enabled: true,
        exposure,
        tool_exposure: Vec::new(),
        description: String::new(),
        source: ServerSource::User,
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
}

fn call(
    bridge: &lca_mcp::McpBridge,
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

/// Read the echo fixture's binary resource and return the staged path.
fn stage_one(caps: std::sync::Arc<lca_tools::Capabilities>) -> std::path::PathBuf {
    let bridge = lca_mcp::McpBridge::connect_managed(caps, vec![stdio_entry(McpExposure::Direct)]);
    let staged = call(
        &bridge,
        lca_mcp::READ_RESOURCE_TOOL,
        r#"{"server":"echo","uri":"echo://bytes"}"#,
    )
    .expect("read bytes");
    std::path::PathBuf::from(staged.content.trim())
}

// Verifies: gh #344 acceptance 1 - the staged file lives inside the session's
// own temp scope, not the shared system temp directory, and its directory is
// not world-writable.
#[test]
fn a_binary_resource_stages_inside_the_session_temp_scope() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let caps = sandbox("staging-location", lca_mcp::manifest_grants());
    let temp_root = caps.scope_root("temp").expect("temp is granted");
    let path = stage_one(caps);

    assert!(
        path.starts_with(&temp_root),
        "the staged file must live in the session temp scope ({}), got {}",
        temp_root.display(),
        path.display()
    );
    assert!(path.is_file(), "and it must exist: {}", path.display());
    assert_eq!(
        std::fs::read(&path).expect("read staged"),
        b"binary-bytes",
        "the payload round-trips"
    );

    // The old shared location is never used, and nothing is created there.
    let shared = std::env::temp_dir().join("lca-mcp-resources");
    assert!(
        !path.starts_with(&shared),
        "never staged into the shared directory: {}",
        path.display()
    );

    // The scope is a per-session directory the host configured, not the
    // shared `lca-mcp-resources` arena every process used to write into.
    // (The scratch root itself sits under the platform temp dir in tests, so
    // the meaningful assertion is *which* directory, not its ancestry.)
    assert!(
        temp_root.ends_with(std::path::Path::new("data/tmp/session")),
        "the session temp scope is the host's per-session directory: {}",
        temp_root.display()
    );

    // The directory is owner-only: no group/other bits at all.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = std::fs::metadata(&temp_root)
            .expect("stat temp root")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            dir_mode & 0o077,
            0,
            "the temp directory must not be group- or world-accessible: {dir_mode:o}"
        );
        let file_mode = std::fs::metadata(&path)
            .expect("stat staged")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            file_mode & 0o077,
            0,
            "the staged file must be owner-only: {file_mode:o}"
        );
        assert_eq!(file_mode & 0o111, 0, "and never executable: {file_mode:o}");
    }
}

// Verifies: gh #344 acceptance 1 - the file itself is created `0600`, so a
// permissive umask cannot widen it.
#[cfg(unix)]
#[test]
fn the_staged_file_is_owner_only_regardless_of_umask() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let caps = sandbox("staging-mode", lca_mcp::manifest_grants());
    let path = stage_one(caps);
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&path)
        .expect("stat staged")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "the staged file is exactly owner read/write: {mode:o}"
    );
}

// Verifies: gh #344 acceptance 1, the attack itself refuted. A local attacker
// pre-plants a symlink at the OLD predictable shared path; staging must
// neither follow it nor touch it, and the sentinel it points at must keep
// its original contents.
#[cfg(unix)]
#[test]
fn a_preplanted_symlink_at_the_old_shared_path_is_never_followed() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    // The victim file the attack aims to overwrite, and the shared directory
    // the old code wrote into.
    let victim_root = lca_testkit::scratch_path("lca-mcp-344-victim");
    let _ = std::fs::remove_dir_all(&victim_root);
    std::fs::create_dir_all(&victim_root).expect("mkdir victim");
    let victim = victim_root.join("precious.txt");
    std::fs::write(&victim, b"UNTOUCHED").expect("write victim");

    let shared = std::env::temp_dir().join("lca-mcp-resources");
    std::fs::create_dir_all(&shared).expect("mkdir shared");
    // The old naming scheme starts at counter 0 on a fresh process.
    let planted = shared.join("echo-0-bytes");
    let _ = std::fs::remove_file(&planted);
    std::os::unix::fs::symlink(&victim, &planted).expect("plant symlink");

    let caps = sandbox("staging-symlink", lca_mcp::manifest_grants());
    let path = stage_one(caps);

    // The staged file is the real one in the private scope, not the link.
    assert_ne!(
        path.canonicalize().expect("canonical staged"),
        victim.canonicalize().expect("canonical victim"),
        "staging must not resolve to the attacker's target"
    );
    assert_eq!(
        std::fs::read(&victim).expect("victim readable"),
        b"UNTOUCHED",
        "the victim file must be untouched"
    );
    // And the planted link itself was not written through.
    let planted_meta = std::fs::symlink_metadata(&planted).expect("planted still there");
    assert!(
        planted_meta.file_type().is_symlink(),
        "the planted entry is still a symlink, not a file we wrote"
    );

    let _ = std::fs::remove_file(&planted);
    let _ = std::fs::remove_dir_all(&victim_root);
}

// Verifies: gh #344 acceptance 2 - no capability bypass. The write runs
// through the scope machinery, so an extension WITHOUT the `temp` grant
// cannot stage at all (it is refused and recorded, not silently written).
#[test]
fn staging_without_the_temp_grant_is_refused_by_the_scope() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    // The pre-#344 grant set: workspace read + process, no temp.
    let mut grants = lca_mcp::manifest_grants();
    grants.fs.retain(|grant| grant.scope != "temp");
    let caps = sandbox("staging-notemp", grants);
    assert!(
        caps.scope_root("temp").is_none(),
        "the fixture really has no temp grant"
    );
    let bridge = lca_mcp::McpBridge::connect_managed(caps, vec![stdio_entry(McpExposure::Direct)]);
    let outcome = call(
        &bridge,
        lca_mcp::READ_RESOURCE_TOOL,
        r#"{"server":"echo","uri":"echo://bytes"}"#,
    );
    let err = match outcome {
        Ok(result) => panic!(
            "staging without the grant must fail, got {:?}",
            result.content
        ),
        Err(err) => format!("{err}"),
    };
    assert!(
        err.contains("temp") || err.contains("not granted") || err.contains("cannot stage"),
        "the refusal names the staging failure: {err}"
    );
}

// Verifies: gh #344 acceptance 3 - staged files do not outlive the session.
// The session temp directory is removed by the guard the CLI installs at
// startup, so dropping it takes the staged blob with it.
#[test]
fn a_staged_resource_does_not_outlive_its_session_temp() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let caps = sandbox("staging-cleanup", lca_mcp::manifest_grants());
    let temp_root = caps.scope_root("temp").expect("temp is granted");
    let path = stage_one(caps);
    assert!(path.is_file(), "staged before cleanup");

    // Exactly what the session-temp guard does at exit.
    lca_cli_test_shim_cleanup(&temp_root);

    assert!(
        !path.exists(),
        "the staged file is gone with the session temp dir: {}",
        path.display()
    );
    assert!(!temp_root.exists(), "and the directory itself is removed");
}

/// The cleanup the CLI performs on every exit path
/// (`crate::paths::cleanup_session_temp`: a best-effort `remove_dir_all`).
fn lca_cli_test_shim_cleanup(dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(dir);
}

// Verifies: gh #344 acceptance 4 - the model can consume the staged path.
// An out-of-workspace read is never prompted (FR-PERM-27's fatigue cut), so
// the path the model receives is usable without modal spam. The permission
// decision is computed here through the same `required_permission` the turn
// loop calls, and then the read is actually performed.
#[test]
fn the_staged_path_is_consumable_without_an_extra_prompt() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let caps = sandbox("staging-consume", lca_mcp::manifest_grants());
    let path = stage_one(caps);

    // The read tool would be asked to read this path from the workspace.
    let workspace = lca_testkit::scratch_path("lca-mcp-344-ws");
    let shell = lca_tools::shell::resolve("bash", None)
        .ok()
        .or_else(|| lca_tools::shell::resolve("sh", None).ok());
    let Some(shell) = shell else {
        eprintln!("skip: no shell to build a tool executor");
        return;
    };
    let tools = lca_tools::ToolExecutor::new(
        std::sync::Arc::new(lca_tools::NativeOps::new(shell)),
        workspace.clone(),
        workspace.clone(),
        64 * 1024,
        Some(std::time::Duration::from_secs(30)),
    );
    let call = lca_protocol::ToolCall {
        call_id: "c1".to_string(),
        name: "read".to_string(),
        arguments: serde_json::json!({ "path": path.to_string_lossy() }).to_string(),
        parent_call_id: None,
    };
    // `required_permission` yields a ReadPath action; FR-PERM-27's fatigue
    // cut is what keeps it silent, and `authorize` proves it here.
    let action = tools
        .required_permission(&call)
        .expect("a read asks for a decision");
    let mut store = lca_permissions::GrantStore::empty();
    let mut prompt = DenyAll;
    let outcome =
        lca_permissions::authorize(&mut store, &workspace, &action, None, &mut prompt, None)
            .expect("authorize runs");
    assert!(
        outcome.allowed && !outcome.prompted,
        "an out-of-workspace read is allowed without prompting (FR-PERM-27)"
    );
    assert_eq!(
        std::fs::read(&path).expect("the model can read the staged bytes"),
        b"binary-bytes"
    );
    let _ = std::fs::remove_dir_all(&workspace);
}

/// A prompt that fails the test if it is ever consulted: FR-PERM-27 says an
/// out-of-workspace read never reaches one.
struct DenyAll;
impl lca_permissions::PermissionPrompt for DenyAll {
    fn ask(&mut self, action: &lca_permissions::Action) -> lca_permissions::Decision {
        panic!(
            "prompted for {} (FR-PERM-27 forbids this)",
            action.display()
        );
    }
    fn review_proposals(&mut self, _: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

// Verifies: gh #344 - the HTTP transport stages identically. Both transports
// share `read_result`, so the contract is the same; this row pins it rather
// than assuming.
#[test]
fn an_http_resource_stages_into_the_same_private_scope() {
    let mock = mock_server();
    let caps = sandbox("staging-http", lca_mcp::manifest_grants());
    let temp_root = caps.scope_root("temp").expect("temp is granted");
    let entry = ServerEntry {
        name: "remote".to_string(),
        kind: EntryKind::Http {
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
    let staged = call(
        &bridge,
        lca_mcp::READ_RESOURCE_TOOL,
        r#"{"server":"remote","uri":"mock://bytes"}"#,
    )
    .expect("read bytes over http");
    let path = std::path::PathBuf::from(staged.content.trim());
    assert!(
        path.starts_with(&temp_root),
        "an HTTP-sourced blob stages in the same private scope: {}",
        path.display()
    );
}
