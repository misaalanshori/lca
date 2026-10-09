//! gh #53 phase 3: `mcp.json` parsing, merging, exposure rules, and
//! the skips that keep one bad entry from blocking the rest.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use mcp::config::{
    McpExposure, apply_project_json, expand_value, exposure_for, parse_mcp_json, pattern_matches,
};

// Verifies: gh #53 - a user file with one stdio and one HTTP server
// parses; names, transports, and knobs land on the entries.
#[test]
fn user_files_parse_both_transports() {
    // SAFETY: test-scoped variable names, set before any read in this
    // test only; no other test touches them.
    unsafe { std::env::set_var("MCP_P3_TEST_DOCS_TOKEN", "token") };
    let parsed = parse_mcp_json(
        r#"{"mcpServers": {
            "echo": {"command": "python3", "args": ["e.py"], "exposure": "direct", "description": "Echo."},
            "docs": {"url": "https://example.com/mcp", "headers": {"Authorization": "Bearer ${MCP_P3_TEST_DOCS_TOKEN}"}, "timeout": 30}
        }}"#,
        mcp::config::ServerSource::User,
    );
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.servers.len(), 2);
    let echo = parsed
        .servers
        .iter()
        .find(|s| s.name == "echo")
        .expect("echo");
    assert!(matches!(echo.kind, mcp::config::EntryKind::Stdio { .. }));
    assert_eq!(echo.exposure, McpExposure::Direct);
    assert_eq!(echo.description, "Echo.");
    let docs = parsed
        .servers
        .iter()
        .find(|s| s.name == "docs")
        .expect("docs");
    assert!(matches!(docs.kind, mcp::config::EntryKind::Http { .. }));
}

// Verifies: gh #53 - invalid entries are reported and skipped without
// blocking the valid ones (bad name, both transports, SSE, unknown
// exposure, `!command` env).
#[test]
fn invalid_entries_report_and_skip() {
    // SAFETY: test-scoped variable names, set before any read in this
    // test only; no other test touches them.
    unsafe { std::env::set_var("MCP_P3_TEST_TOKEN", "secret") };
    let parsed = parse_mcp_json(
        r#"{"mcpServers": {
            "ok": {"command": "true"},
            "bad name": {"command": "true"},
            "both": {"command": "true", "url": "https://x.invalid/m"},
            "sse": {"type": "sse", "url": "https://x.invalid/s"},
            "wtf": {"command": "true", "exposure": "sometimes"},
            "bang": {"command": "true", "env": {"K": "!run me"}}
        }}"#,
        mcp::config::ServerSource::User,
    );
    assert_eq!(parsed.servers.len(), 1);
    assert_eq!(parsed.servers[0].name, "ok");
    assert_eq!(parsed.warnings.len(), 5, "{:?}", parsed.warnings);
}

// Verifies: gh #53 - a project replacement swaps the transport, a
// partial override flips knobs only, and an unknown override skips.
#[test]
fn project_files_replace_or_patch() {
    let mut user = parse_mcp_json(
        r#"{"mcpServers": {
            "echo": {"command": "python3", "exposure": "direct"},
            "keep": {"command": "true"}
        }}"#,
        mcp::config::ServerSource::User,
    );
    apply_project_json(
        &mut user,
        r#"{"mcpServers": {
            "echo": {"enabled": false, "exposure": "deferred"},
            "keep": {"url": "https://example.com/mcp"},
            "ghost": {"enabled": false}
        }}"#,
    );
    assert_eq!(user.warnings.len(), 1, "{:?}", user.warnings);
    let echo = user
        .servers
        .iter()
        .find(|s| s.name == "echo")
        .expect("echo");
    assert!(!echo.enabled);
    assert_eq!(echo.exposure, McpExposure::Deferred);
    assert!(matches!(echo.kind, mcp::config::EntryKind::Stdio { .. }));
    assert_eq!(echo.source, mcp::config::ServerSource::Project);
    let keep = user
        .servers
        .iter()
        .find(|s| s.name == "keep")
        .expect("keep");
    assert!(matches!(keep.kind, mcp::config::EntryKind::Http { .. }));
}

// Verifies: gh #53 - exposure resolution: exact names win, then the
// first pattern, then the default; `${VAR}` expands and `!` refuses.
#[test]
fn exposure_and_env_shapes_hold() {
    let rules = vec![
        ("get_*".to_string(), McpExposure::Codemode),
        ("delete_*".to_string(), McpExposure::Hidden),
    ];
    assert_eq!(
        exposure_for(McpExposure::Deferred, &rules, "get_thing"),
        McpExposure::Codemode
    );
    assert_eq!(
        exposure_for(McpExposure::Deferred, &rules, "delete_all"),
        McpExposure::Hidden
    );
    assert_eq!(
        exposure_for(McpExposure::Deferred, &rules, "other"),
        McpExposure::Deferred
    );
    assert!(pattern_matches("get_*", "get_x"));
    assert!(!pattern_matches("get_*", "forget_x"));
    // SAFETY: test-scoped variable names, set before any read in this
    // test only; no other test touches them.
    unsafe { std::env::set_var("MCP_P3_TEST_HOME", "/home/tester") };
    assert_eq!(
        expand_value("~/x ${MCP_P3_TEST_HOME}").as_deref(),
        Ok("~/x /home/tester")
    );
    assert!(expand_value("!run me").is_err());
    assert!(expand_value("${MCP_P3_DEFINITELY_UNSET_VAR}").is_err());
}
