//! gh #53 phase 2: the streamable-HTTP transport against the mock
//! remote - list, call, SSE envelopes, session ids, transient retry,
//! and the per-request timeout.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod common;

use common::{mock_server, remote_server, sandbox};

// Verifies: gh #53 - a remote server lists its echo tool and answers
// a call through the bridge, over plain POSTs.
#[test]
fn a_remote_server_lists_and_calls_echo() {
    let mock = mock_server();
    let caps = sandbox("remote-echo", lca_mcp::manifest_grants());
    let bridge = lca_mcp::McpBridge::connect_http(caps, vec![remote_server(&mock, "/mcp")])
        .expect("connect");
    let specs = bridge.tool_specs().expect("specs");
    assert_eq!(specs.len(), 4, "echo plus the resource trio");
    assert!(specs.iter().any(|spec| spec.name == "mcp__remote__echo"));
    let result = runtime().block_on(bridge.execute_tool(&lca_protocol::ToolCall {
        call_id: "call-1".to_string(),
        name: "mcp__remote__echo".to_string(),
        arguments: r#"{"text":"hello"}"#.to_string(),
        parent_call_id: None,
    }));
    let result = result.expect("run");
    assert_eq!(result.status, lca_protocol::ToolResultStatus::Ok);
    assert_eq!(result.content, "echo: hello");
    let posts = mock.requests_of("POST /mcp");
    assert!(
        posts
            .iter()
            .any(|request| request.contains("application/json")),
        "JSON posts: {posts:?}"
    );
}

// Verifies: gh #53 - two transient failures retry (pi's delays) and
// the third attempt succeeds.
#[test]
fn transient_failures_retry_then_succeed() {
    let mock = mock_server();
    mock.fail_once("/mcp", 503, None, Vec::new());
    mock.fail_once("/mcp", 503, None, Vec::new());
    let caps = sandbox("remote-retry", lca_mcp::manifest_grants());
    let bridge = lca_mcp::McpBridge::connect_http(caps, vec![remote_server(&mock, "/mcp")])
        .expect("connect");
    let result = runtime().block_on(bridge.execute_tool(&lca_protocol::ToolCall {
        call_id: "call-1".to_string(),
        name: "mcp__remote__echo".to_string(),
        arguments: r#"{"text":"again"}"#.to_string(),
        parent_call_id: None,
    }));
    assert_eq!(result.expect("run").content, "echo: again");
    // initialize + list + one call, each attempted until it lands:
    // the two scripted 503s burned two attempts of one of them.
    let posts = mock.requests_of("POST /mcp ");
    assert!(posts.len() >= 4, "retried posts: {posts:?}");
}

// Verifies: gh #53 - a server that never answers fails the call
// inside the configured timeout instead of hanging the turn.
#[test]
fn a_hung_server_times_out() {
    let mock = mock_server();
    let caps = sandbox("remote-timeout", lca_mcp::manifest_grants());
    let mut server = remote_server(&mock, "/slow");
    server.timeout_secs = 1;
    let configured = server.timeout_secs;
    let started = std::time::Instant::now();
    let err = match lca_mcp::McpBridge::connect_http(caps, vec![server]) {
        Ok(_) => panic!("slow connect fails"),
        Err(err) => err,
    };
    assert!(
        err.contains("timed out"),
        "the error names the timeout: {err}"
    );
    // Gh #609: the `< 10 s` stopwatch is replaced by a bound **composed of the value
    // the caller configured**. The bridge makes up to three bounded exchanges on this
    // path, so its cost is a multiple of `timeout_secs` rather than one timeout:
    // measured, 1 s configured costs 4.26 s and 2 s costs 7.26 s - slope 3x plus the
    // fixture's own ~1.3 s delay. The bound is that arithmetic with room: `4 x
    // configured + 2 s`. A fixed 10 s (what this row had) would be *too tight* for a
    // 3 s configuration and too loose for a 1 s one, which is why the number follows
    // the configuration now - and a bridge that ignored `timeout_secs` and used the
    // 60 s default (about 180 s here) still reddens, while the effect assertion above
    // is what catches a bridge that never gave up at all.
    //
    // What this cannot prove, stated rather than implied: the fixture's own delay means
    // a bridge that reported "timed out" *without* waiting would still land inside this
    // bound (measured with a zero timeout: 1.4 s). The identity `timeout_secs ->
    // Duration` has its own rows in `types.rs`.
    let bound = std::time::Duration::from_secs(configured * 4 + 2);
    assert!(
        started.elapsed() < bound,
        "the wait is bounded by 4x the configured {configured}s timeout plus the \
         fixture's delay ({bound:?}), not by the 60 s default: {:?}",
        started.elapsed()
    );
}

// Verifies: gh #53 - an SSE envelope reads the same as a JSON one.
#[test]
fn sse_responses_read() {
    let mock = mock_server();
    let caps = sandbox("remote-sse", lca_mcp::manifest_grants());
    let bridge = lca_mcp::McpBridge::connect_http(caps, vec![remote_server(&mock, "/mcp-sse")])
        .expect("connect");
    let specs = bridge.tool_specs().expect("specs");
    assert_eq!(specs.len(), 4, "echo plus the resource trio");
    assert!(specs.iter().any(|spec| spec.name == "mcp__remote__echo"));
}

// Verifies: gh #53 - the server's session id rides every later post.
#[test]
fn session_id_round_trips() {
    let mock = mock_server();
    let caps = sandbox("remote-session", lca_mcp::manifest_grants());
    let bridge = lca_mcp::McpBridge::connect_http(caps, vec![remote_server(&mock, "/mcp")])
        .expect("connect");
    let posts = mock.requests_of("POST /mcp");
    assert!(posts.len() >= 2, "initialize + list posted: {posts:?}");
    assert!(
        !posts[0].to_ascii_lowercase().contains("mcp-session-id"),
        "no session yet on the first post"
    );
    assert!(
        posts[1..]
            .iter()
            .all(|post| post.contains("mock-session-1")),
        "later posts carry it: {posts:?}"
    );
    let _ = bridge;
}

// Verifies: gh #53 - a `tools/call` never retries: one transient
// answer is the error result (the server may already have run it).
#[test]
fn a_call_reports_transient_failure_without_retry() {
    let mock = mock_server();
    let caps = sandbox("remote-no-retry", lca_mcp::manifest_grants());
    let bridge = lca_mcp::McpBridge::connect_http(caps, vec![remote_server(&mock, "/mcp")])
        .expect("connect");
    mock.fail_once("tools/call", 503, None, Vec::new());
    let result = runtime().block_on(bridge.execute_tool(&lca_protocol::ToolCall {
        call_id: "call-1".to_string(),
        name: "mcp__remote__echo".to_string(),
        arguments: r#"{"text":"once"}"#.to_string(),
        parent_call_id: None,
    }));
    // A transport failure is a dispatch failure (the tool never ran),
    // not an error result: the turn records the extension event.
    let err = match result {
        Ok(result) => panic!("a failed call is no ok result: {result:?}"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("503"), "the status survives: {err}");
    let calls = mock.requests_of("tools/call");
    assert_eq!(calls.len(), 1, "exactly one attempt: {calls:?}");
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
}
