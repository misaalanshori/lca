//! gh #418 Phase C#C4 (gh #267): `mcp.json` writes are atomic and
//! owner-only, and `set_exposure`'s confirmation names the server.
//!
//! The atomicity row follows the house precedent (`lca-tools`
//! `state_write`): write a sibling temporary file, set `0600` on Unix, then
//! `rename` over the target. A crash between the write and the rename leaves
//! the previous file untouched, which is the property that matters - a
//! half-written `mcp.json` is a config the user loses.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_mcp::config::{ServerEntry, ServerSource, parse_mcp_json};
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

fn root(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(&format!("lca-mcp-c4-{name}"));
    for part in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(dir.join(part)).expect("mkdir");
    }
    dir
}

/// A manager over one user-level server, writing into `data/mcp.json`.
fn manager_with(dir: &std::path::Path, name: &str) -> lca_mcp::McpManager {
    let user_path = dir.join("data").join("mcp.json");
    std::fs::write(
        &user_path,
        format!(r#"{{"mcpServers": {{"{name}": {{"command": "true"}}}}}}"#),
    )
    .expect("seed");
    lca_mcp::McpManager::load(engine(dir), user_path, None, vec![entry(name)])
}

fn entry(name: &str) -> ServerEntry {
    parse_mcp_json(
        &format!(r#"{{"mcpServers": {{"{name}": {{"command": "true"}}}}}}"#),
        ServerSource::User,
    )
    .servers
    .into_iter()
    .next()
    .expect("one entry")
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

// Verifies: gh #418 Phase C#C4 (gh #267) - the write goes through a sibling
// temporary file that is renamed over the target, so the file on disk is
// never half-written, and no `.tmp` sibling is left behind on success.
#[test]
fn an_mcp_json_write_lands_by_rename_and_leaves_no_temp_file() {
    let dir = root("atomic");
    let path = dir.join("data").join("mcp.json");
    let manager = manager_with(&dir, "alpha");
    assert!(path.is_file(), "the seed target exists");
    manager.act("disable", "alpha");
    manager.act("exposure", "alpha direct");

    // The target parses and holds the server; no temporary sibling survives.
    let text = std::fs::read_to_string(&path).expect("read");
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert!(parsed["mcpServers"]["alpha"].is_object(), "{text}");

    let leftovers: Vec<String> = std::fs::read_dir(path.parent().expect("parent"))
        .expect("read dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "the rename consumes the temporary file: {leftovers:?}"
    );
}

// Verifies: gh #418 Phase C#C4 (gh #267) - the written file is owner-only on
// Unix. `mcp.json` holds credentials in `env` and `headers` (tokens live
// there by design), so the process's umask must not decide who can read it.
#[cfg(unix)]
#[test]
fn an_mcp_json_write_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = root("mode");
    let path = dir.join("data").join("mcp.json");
    let manager = manager_with(&dir, "alpha");
    manager.act("disable", "alpha");
    let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "mcp.json is owner-only, got {mode:o}");
}

// Verifies: gh #418 Phase C#C4 (gh #267) - the file's own indentation is
// preserved, so a hand-formatted config keeps its shape across an edit
// instead of being rewritten to the host's default.
#[test]
fn an_mcp_json_write_preserves_the_files_indentation() {
    let dir = root("indent");
    let path = dir.join("data").join("mcp.json");
    std::fs::write(
        &path,
        "{\n    \"mcpServers\": {\n        \"alpha\": {\"command\": \"true\"}\n    }\n}\n",
    )
    .expect("seed");
    let manager = lca_mcp::McpManager::load(engine(&dir), path.clone(), None, vec![entry("alpha")]);
    // Re-enabling writes the full entry, which is the write under test.
    manager.act("enable", "alpha");
    let text = std::fs::read_to_string(&path).expect("read");
    assert!(
        text.contains("\n    \"mcpServers\"") || text.contains("\n        \"alpha\""),
        "the four-space indentation survives: {text}"
    );
    assert!(
        !text.contains("\n  \"mcpServers\""),
        "never rewritten to two spaces: {text}"
    );
}

// Verifies: gh #418 Phase C#C4 (gh #267) - `set_exposure` names the server,
// not the raw argument string. Before the fix, `exposure echo direct` replied
// with `` `echo direct` exposure: direct ``, echoing the whole argument.
#[test]
fn the_exposure_confirmation_names_the_server_not_the_argument() {
    let dir = root("copy");
    let manager = manager_with(&dir, "echo");
    let reply = manager.act("exposure", "echo codemode");
    assert_eq!(
        reply, "`echo` exposure: codemode",
        "the confirmation names the server, not the argument"
    );
    // The cycle form (no level word) names it too.
    let reply = manager.act("exposure", "echo");
    assert_eq!(
        reply, "`echo` exposure: deferred",
        "the cycle form also names the server"
    );
}
