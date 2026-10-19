//! The MCP bridge's manifest and its native grant table must not drift.
//!
//! `extensions/mcp/extension.toml` is what the host parses, validates, and
//! shows the user on the consent screen; `mcp::manifest_grants()` is what the
//! native (compiled-in) delivery mode hands the capability engine without
//! reading a manifest at all. Two hand-maintained declarations of one
//! capability set, in two formats, in two files.
//!
//! Every other built-in extension has the same pair, and until now nothing
//! compared them: a grant added to the TOML would silently not exist in native
//! mode, and a grant added to the Rust would silently not appear on the
//! consent screen. gh #344 changed both at once (adding `temp = "read-write"`
//! for resource staging), which is exactly the moment the two can diverge
//! without anyone noticing.
//!
//! Verifies: gh #344 follow-up WO2 - `[capabilities.fs]` and
//! `manifest_grants().fs` declare the identical scope set, and `process` is
//! declared in both.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::Path;

/// The `mcp` crate's own directory, which holds `extension.toml`.
fn package_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Parse the manifest the host would read.
fn manifest() -> toml::Table {
    let text = std::fs::read_to_string(package_dir().join("extension.toml"))
        .expect("extensions/mcp/extension.toml is present");
    text.parse::<toml::Table>()
        .expect("the MCP manifest is valid TOML")
}

/// `[capabilities.fs]` as (scope, mode) pairs, sorted so the assertion does not
/// depend on table order.
fn manifest_fs() -> Vec<(String, String)> {
    let table = manifest();
    let fs = table
        .get("capabilities")
        .and_then(|c| c.get("fs"))
        .and_then(|f| f.as_table())
        .expect("[capabilities.fs] is a table");
    let mut pairs: Vec<(String, String)> = fs
        .iter()
        .map(|(scope, mode)| {
            (
                scope.clone(),
                mode.as_str()
                    .unwrap_or_else(|| panic!("fs scope `{scope}` carries a string mode"))
                    .to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// The native grant table as the same (scope, mode) pairs.
fn native_fs() -> Vec<(String, String)> {
    let grants = mcp::manifest_grants();
    let mut pairs: Vec<(String, String)> = grants
        .fs
        .iter()
        .map(|grant| {
            let mode = match grant.mode {
                lca_permissions::FsMode::Read => "read",
                lca_permissions::FsMode::ReadWrite => "read-write",
            };
            (grant.scope.to_string(), mode.to_string())
        })
        .collect();
    pairs.sort();
    pairs
}

/// The exact set the bridge needs, stated once so a drift in *either*
/// direction fails here with a specific message: `workspace` read for the
/// server's working directory, `temp` read-write for resource staging
/// (gh #344).
#[test]
fn the_manifest_declares_exactly_the_expected_scopes() {
    assert_eq!(
        manifest_fs(),
        vec![
            ("temp".to_string(), "read-write".to_string()),
            ("workspace".to_string(), "read".to_string()),
        ],
        "extension.toml's [capabilities.fs] is workspace=read + temp=read-write"
    );
}

/// The parity row: whatever the manifest says, the native table says the same.
#[test]
fn the_native_grants_match_the_manifest() {
    assert_eq!(
        native_fs(),
        manifest_fs(),
        "manifest_grants() and extension.toml must declare the identical fs set; \
         a grant present in one and not the other means the compiled-in mode and \
         the installed mode disagree about what the bridge may reach"
    );
}

/// `process` is declared on both sides - the bridge cannot spawn a server
/// without it, and a manifest that forgot it would fail at spawn time instead
/// of at install time.
#[test]
fn process_is_declared_in_both() {
    let table = manifest();
    assert!(
        table
            .get("capabilities")
            .and_then(|c| c.get("process"))
            .is_some(),
        "extension.toml declares [capabilities.process]"
    );
    assert!(
        mcp::manifest_grants().process,
        "manifest_grants() declares process"
    );
}

/// Both sides declare `fs` at all. `fs_declared` is what separates "declared,
/// no scopes yet" from "never declared" (gh #302's lesson), so a native table
/// with the right vector but the flag off would still refuse every path.
#[test]
fn fs_is_marked_declared_on_both_sides() {
    assert!(
        !manifest_fs().is_empty(),
        "the manifest declares at least one fs scope"
    );
    assert!(
        mcp::manifest_grants().fs_declared,
        "manifest_grants() sets fs_declared, or every path is refused as undeclared"
    );
}
