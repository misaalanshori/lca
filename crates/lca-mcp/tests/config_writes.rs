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

// Verifies: gh #267 - **a failed write leaves the previous `mcp.json` intact.**
//
// This is the property the issue is about, and the earlier rows did not assert
// it: they checked that a *successful* write lands by rename and leaves no temp
// file, which a hand-rolled in-place `fs::write` would also pass on the happy
// path. What matters is the unhappy one - `mcp.json` holds server configs,
// environment variables and authentication headers, so a truncated file is a
// config the user loses and credentials they have to re-enter.
//
// Fault injection, not code inspection: the target is made unreadable-for-write
// by turning the *directory* read-only, which fails the temp-file creation
// before anything can touch the target. The assertion is "byte-identical", not
// "no partial file": the old content has to survive.
#[cfg(unix)]
#[test]
fn a_failed_write_leaves_the_previous_mcp_json_byte_identical() {
    use std::os::unix::fs::PermissionsExt;
    let dir = root("failed-write");
    let path = dir.join("data").join("mcp.json");
    let manager = manager_with(&dir, "alpha");
    // A distinctive body, so "intact" cannot be satisfied by an empty file.
    let original = std::fs::read(&path).expect("read the seeded file");
    assert!(!original.is_empty(), "the fixture has content to lose");

    // Freeze the directory: the sibling temp file cannot be created, so the
    // write fails before the rename and the target is never opened.
    let dir_path = path.parent().expect("parent").to_path_buf();
    std::fs::set_permissions(&dir_path, std::fs::Permissions::from_mode(0o500)).expect("chmod 500");
    let reply = manager.act("disable", "alpha");
    std::fs::set_permissions(&dir_path, std::fs::Permissions::from_mode(0o700))
        .expect("chmod back");

    // The write did fail - otherwise this row proves nothing about failure.
    assert!(
        reply.to_lowercase().contains("cannot")
            || reply.to_lowercase().contains("denied")
            || reply.to_lowercase().contains("error"),
        "the frozen directory made the write fail: {reply}"
    );
    let after = std::fs::read(&path).expect("the target still reads");
    assert_eq!(
        after, original,
        "the previous mcp.json survives a failed write byte for byte"
    );
    // And no temporary sibling was left behind by the failed attempt.
    let leftovers: Vec<String> = std::fs::read_dir(&dir_path)
        .expect("read dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("tmp"))
        .collect();
    assert!(leftovers.is_empty(), "no temp file survives: {leftovers:?}");
}

// Verifies: gh #267 - the temp file is restricted **before** the rename, so
// there is no window in which `mcp.json` exists world-readable.
//
// The assertion available to a test is the mode the mode-setting step leaves on
// the sibling: the code sets it after `write` and before `rename`, and a rename
// does not change a file's permissions. So "the target is 0600" plus "the write
// is a create-then-chmod on the same inode" is the strongest observable claim;
// the pre-rename window itself cannot be sampled from a single-threaded test,
// which is stated rather than pretended.
#[cfg(unix)]
#[test]
fn the_temp_file_is_restricted_before_it_is_published() {
    use std::os::unix::fs::PermissionsExt;
    let dir = root("pre-rename-mode");
    let path = dir.join("data").join("mcp.json");
    let manager = manager_with(&dir, "alpha");

    // A readable-by-group seed proves the write does not simply inherit the
    // target's mode: the result is 0600 even though the file it replaces was not.
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
    manager.act("disable", "alpha");
    let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "the published file is owner-only even when the old one was 0644, which is \
         only possible if the mode was set on the temp file before the rename"
    );

    // The source shape: the chmod happens between the write and the rename. A
    // chmod *after* the rename would leave a world-readable window.
    let source =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/config.rs"))
            .expect("read config.rs");
    let body = {
        let start = source.find("fn write_json_file").expect("the helper");
        let rest = &source[start..];
        let end = rest.find("\n}\n").expect("its end");
        &rest[..end]
    };
    let write_at = body.find("std::fs::write(&temp").expect("the temp write");
    let chmod_at = body.find("set_permissions(&temp").expect("the temp chmod");
    let rename_at = body.find("std::fs::rename(&temp").expect("the rename");
    assert!(
        write_at < chmod_at && chmod_at < rename_at,
        "write, then restrict, then publish - a chmod after the rename is a \
         world-readable window: {body}"
    );
}

// Verifies: gh #267 - the confirmation prints the **parsed** server name, not
// the raw argument, in every case where the two differ.
//
// The original defect printed `target` — the whole argument string — instead of
// the `name` field it had just parsed out of it. So the row that proves the fix
// has to be a case where those two strings differ, and there are exactly two
// shapes available:
//
// 1. **the level form** `exposure <name> <level>`, where the argument is
//    `"alpha direct"` and the name is `"alpha"`;
// 2. **a name that collides with a level keyword** — `direct` is a legal server
//    name and also a legal level, so `exposure direct hidden` must resolve to
//    the *server* called `direct`, not to the server called `direct hidden`.
//
// **The brief asked for a server name containing a space, and that case cannot
// exist here**: `lca_mcp::is_valid_server_name` accepts only ASCII
// alphanumerics, `_` and `-`, so `"filesystem direct"` as a *name* is rejected at
// parse time and the fixture would test nothing. The level-keyword collision is
// the strongest available substitute: it is a real, loadable server whose name is
// also the argument's last word.
#[test]
fn the_exposure_confirmation_prints_the_parsed_name_not_the_argument() {
    // Shape 1: the level form. Argument `alpha direct`, name `alpha`.
    let dir = root("parsed-name-level");
    let manager = manager_with(&dir, "alpha");
    assert_eq!(
        manager.act("exposure", "alpha direct"),
        "`alpha` exposure: direct",
        "the level form prints the parsed name, not `alpha direct`"
    );

    // Shape 2: a server whose name *is* a level keyword. The argument is
    // `direct hidden`; resolving by the raw string would look for a server named
    // `direct hidden` and find nothing.
    let dir = root("parsed-name-keyword");
    let manager = manager_with(&dir, "direct");
    assert_eq!(
        manager.act("exposure", "direct hidden"),
        "`direct` exposure: hidden",
        "a name that collides with a level word still resolves to itself"
    );
    let text = std::fs::read_to_string(dir.join("data").join("mcp.json")).expect("read");
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(
        parsed["mcpServers"]["direct"]["exposure"], "hidden",
        "and the write targeted that server: {text}"
    );

    // Shape 3: a name that merely *contains* a level word, to show the split is
    // on the last space and not on a substring.
    let dir = root("parsed-name-substring");
    let manager = manager_with(&dir, "codemode-tools");
    assert_eq!(
        manager.act("exposure", "codemode-tools codemode"),
        "`codemode-tools` exposure: codemode",
        "a name containing a level word is still printed whole"
    );
}

// Verifies: gh #267 - a spaced server name is refused at parse time, which is
// why the row above cannot use one. Pinned so the substitute is justified by
// behaviour rather than by a comment.
#[test]
fn a_server_name_with_a_space_is_not_a_valid_name() {
    assert!(!lca_mcp::is_valid_server_name("filesystem direct"));
    assert!(lca_mcp::is_valid_server_name("filesystem-direct"));
    assert!(lca_mcp::is_valid_server_name("direct"));
}
