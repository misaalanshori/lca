//! MCP's native capability grants stay aligned with its extension manifest.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

#[test]
fn manifest_grants_match_workspace_read_and_session_temp_write() {
    let manifest: toml::Value =
        toml::from_str(include_str!("../extension.toml")).expect("MCP manifest parses");
    let fs = manifest
        .get("capabilities")
        .and_then(|value| value.get("fs"))
        .expect("fs capability exists");
    assert_eq!(
        fs.get("workspace").and_then(toml::Value::as_str),
        Some("read")
    );
    assert_eq!(
        fs.get("temp").and_then(toml::Value::as_str),
        Some("read-write")
    );

    let grants = mcp::manifest_grants();
    assert!(grants.process, "the stdio server grant remains declared");
    assert_eq!(
        grants.fs,
        vec![
            lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::Read)
                .expect("workspace grant"),
            lca_permissions::ScopeGrant::parse("temp", lca_permissions::FsMode::ReadWrite)
                .expect("temp grant"),
        ]
    );
}
