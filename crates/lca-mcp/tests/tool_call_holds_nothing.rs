//! gh #554: an MCP tool call never holds the server table and never blocks the
//! runtime.
//!
//! ## The defect
//!
//! `McpBridge::execute_tool` was an `async fn` with **no `.await`** that took
//! `self.locked()` and then ran the whole stdio/HTTP round trip with the guard
//! alive - up to `server.timeout()`, 60 s by default. Everything that reads the
//! table waited behind it: `/mcp`'s rows, the status line, a settings change, and
//! a **reconnect** (which shuts every row down, and therefore kills its child).
//! So a slow tool call froze the interface and made a cancel wait. The retry
//! backoff in `remote.rs` was a plain `std::thread::sleep`, so a cancel during it
//! waited the sleeps out.
//!
//! ## What these rows pin
//!
//! Behaviour only: no lock introspection, and no wall-clock assertion except in
//! the retry row, where the claim *is* about elapsed time and the row says so.
//! Every other wait is a handshake the row owns or a hang guard whose message
//! names which side stalled.
//!
//! The fixture is `probe-mcp-server.py` with `LCA_PROBE_CALL_DELAY`, which keeps
//! `tools/call` genuinely in flight for a known window - a real child answering
//! real JSON-RPC, so nothing is mocked at the seam that broke.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lca_mcp::config::{EntryKind, McpExposure, ServerEntry, ServerSource};
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

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn scratch(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(&format!("lca-mcp-554-{name}"));
    for part in ["project", "private", "config", "data/tmp/session"] {
        std::fs::create_dir_all(dir.join(part)).expect("mkdir");
    }
    dir
}

fn engine(root: &Path) -> Arc<lca_tools::Capabilities> {
    Arc::new(lca_tools::Capabilities::new(
        "mcp",
        lca_mcp::manifest_grants(),
        ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("data/tmp/session"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("grants"),
        )),
        root.join("project"),
        None,
    ))
}

/// A fixture-backed stdio entry; `delay_secs` is the `tools/call` window.
fn entry(name: &str, fixture_name: &str, delay_secs: f64) -> ServerEntry {
    let mut env = std::collections::BTreeMap::new();
    env.insert("LCA_PROBE_CALL_DELAY".to_string(), delay_secs.to_string());
    ServerEntry {
        name: name.to_string(),
        kind: EntryKind::Stdio {
            command: "python3".to_string(),
            args: vec![fixture(fixture_name).to_string_lossy().into_owned()],
            cwd: None,
            env,
        },
        enabled: true,
        exposure: McpExposure::Direct,
        tool_exposure: Vec::new(),
        description: String::new(),
        source: ServerSource::User,
    }
}

/// One tool call, on a runtime of its own (each row owns its threading).
fn call(bridge: &lca_mcp::McpBridge, tool: &str, arguments: &str) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("caller runtime");
    rt.block_on(bridge.execute_tool(&lca_protocol::ToolCall {
        call_id: "call-554".to_string(),
        name: tool.to_string(),
        arguments: arguments.to_string(),
        parent_call_id: None,
    }))
    .map(|result| result.content)
    .map_err(|err| format!("{err:?}"))
}

/// The hang guard: it fires only when the row would otherwise hang, and it names
/// the side that stalled so the next firing is diagnosable in one read.
fn guard<T>(rx: &mpsc::Receiver<T>, what: &str) -> T {
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(value) => value,
        Err(_) => panic!(
            "{what} never completed within 10 s while an MCP call was in flight \
             (gh #554): the call is holding the server table across its exchange"
        ),
    }
}

// Verifies: gh #554 - a call in flight does not block a reader of the table.
//
// `statuses()` and `tool_specs()` both take the table. Before the fix they waited
// for the whole call; now the call holds nothing, so they answer while the call
// is genuinely mid-exchange. The clock is a hang guard plus one reported number -
// the assertion is "the read beat the call's own 2 s delay", which is a fact about
// ordering, not about speed.
#[test]
fn a_call_in_flight_does_not_block_a_table_read() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = scratch("table-read");
    let bridge = Arc::new(lca_mcp::McpBridge::connect_managed(
        engine(&root),
        vec![entry("probe", "probe-mcp-server.py", 3.0)],
    ));

    let (started_tx, started_rx) = mpsc::channel();
    let caller = {
        let bridge = Arc::clone(&bridge);
        let started_tx = started_tx.clone();
        std::thread::spawn(move || {
            started_tx.send(()).expect("handshake");
            call(&bridge, "mcp__probe__probe", "{}")
        })
    };
    started_rx.recv().expect("the caller started");
    // The call is inside `tools/call` for 3 s, so a read issued now has ~2.94 s
    // of blocking ahead of it if the table is held. This pause only makes the
    // read land *inside* that window; the fixture's delay is what makes the row
    // decisive, not the pause.
    std::thread::sleep(Duration::from_millis(60));

    let read_started = Instant::now();
    let (read_tx, read_rx) = mpsc::channel();
    {
        let bridge = Arc::clone(&bridge);
        std::thread::spawn(move || {
            let statuses = bridge.statuses();
            let specs = bridge.tool_specs().map(|specs| specs.len());
            let _ = read_tx.send((statuses.len(), specs, read_started.elapsed()));
        });
    }
    let (servers, specs, elapsed) = guard(&read_rx, "the table read (statuses + tool_specs)");
    assert_eq!(servers, 1, "the row is there");
    assert_eq!(specs.expect("specs listed"), 1, "and its tool is listed");
    assert!(
        elapsed < Duration::from_secs(1),
        "the read returned in {elapsed:?} against ~2.94 s of remaining delay, so \
         it did not wait for the exchange; a table held across the call would \
         block it for the whole remainder"
    );

    let answer = caller.join().expect("the caller thread");
    assert!(answer.is_ok(), "and the call still answers: {answer:?}");
    eprintln!("the table read returned in {elapsed:?} during a 3 s in-flight call");
}

// Verifies: gh #554 - a second call is served while the first is in flight.
//
// Two servers, so the per-server session locks are unrelated: what the second
// call needs from the shared table is its own lookup. Before the fix that lookup
// waited for the first call's whole exchange. The fast server's answer, arriving
// before the slow one's, is the ordering this pins.
#[test]
fn two_servers_are_served_concurrently() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = scratch("two-servers");
    let bridge = Arc::new(lca_mcp::McpBridge::connect_managed(
        engine(&root),
        vec![
            entry("slow", "probe-mcp-server.py", 2.0),
            entry("fast", "echo-mcp-server.py", 0.0),
        ],
    ));
    let (started_tx, started_rx) = mpsc::channel();
    let (order_tx, order_rx) = mpsc::channel();
    let slow = {
        let bridge = Arc::clone(&bridge);
        let order_tx = order_tx.clone();
        std::thread::spawn(move || {
            started_tx.send(()).expect("handshake");
            let answer = call(&bridge, "mcp__slow__probe", "{}");
            let _ = order_tx.send(("slow", answer.is_ok()));
            answer
        })
    };
    started_rx.recv().expect("the slow call started");
    std::thread::sleep(Duration::from_millis(60));

    let fast = {
        let bridge = Arc::clone(&bridge);
        let order_tx = order_tx.clone();
        std::thread::spawn(move || {
            let answer = call(&bridge, "mcp__fast__echo", r#"{"text":"quick"}"#);
            let _ = order_tx.send(("fast", answer.is_ok()));
            answer
        })
    };
    // The first arrival must be the fast server: the slow one still has ~2 s of
    // its own delay left.
    let (first, first_ok) = guard(&order_rx, "the first of two concurrent calls");
    assert!(first_ok, "the first arrival answered: {first}");
    assert_eq!(
        first, "fast",
        "the fast server answered first, so the second call was not queued behind \
         the first one's exchange"
    );
    let fast_answer = fast.join().expect("the fast thread");
    assert_eq!(fast_answer.expect("fast answers"), "echo: quick");
    let slow_answer = slow.join().expect("the slow thread");
    assert!(slow_answer.is_ok(), "the slow call still answers");
    let second = order_rx.try_recv().expect("the slow call finished too");
    assert_eq!(second.0, "slow");
}

// Verifies: gh #554 - a reconnect kills its server while a call is in flight.
//
// This is the cancellation-shaped half: `/mcp reconnect` shuts the rows down,
// which calls `process_kill`. Before the fix that needed the table the in-flight
// call was holding, so a reconnect - and therefore a cancel - waited for the
// exchange. The row drives the manager (the product's own verb) rather than
// reaching into the bridge, and the call then reports the death instead of hanging.
#[test]
fn a_reconnect_does_not_wait_for_an_in_flight_call() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = scratch("reconnect");
    let manager = lca_mcp::McpManager::load(
        engine(&root),
        root.join("data/mcp.json"),
        None,
        vec![entry("probe", "probe-mcp-server.py", 3.0)],
    );
    let pending = manager.wait_until_settled(Duration::from_secs(30));
    assert!(pending.is_empty(), "the slow server connects: {pending:?}");
    let bridge = manager.bridge();

    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let caller = {
        let bridge = Arc::clone(&bridge);
        std::thread::spawn(move || {
            started_tx.send(()).expect("handshake");
            let answer = call(&bridge, "mcp__probe__probe", "{}");
            let _ = done_tx.send(answer);
        })
    };
    started_rx.recv().expect("the call started");
    // The call is inside `tools/call` for 3 s. The reconnect must not wait for it.
    std::thread::sleep(Duration::from_millis(80));

    let reconnect_started = Instant::now();
    let (act_tx, act_rx) = mpsc::channel();
    {
        // `McpManager` is shared behind the caller's `Arc` in the product; here the
        // row owns it, so the reconnect runs on its own thread against a borrowed
        // handle. It is the same entry point `/mcp reconnect` uses.
        let manager: Arc<lca_mcp::McpManager> = Arc::new(manager);
        std::thread::spawn(move || {
            // A global reconnect rebuilds every row: shutdown, then reconnect.
            let _ = act_tx.send(manager.act("reconnect", ""));
        });
    }
    let notice = guard(&act_rx, "the reconnect (manager.act)");
    let reconnect_cost = reconnect_started.elapsed();
    assert!(
        !notice.is_empty(),
        "the reconnect reports something rather than nothing"
    );
    assert!(
        reconnect_cost < Duration::from_secs(3),
        "the reconnect took {reconnect_cost:?}, which means it waited for the \
         in-flight call's own 3 s delay"
    );

    // The call's server was killed underneath it, so it reports a failure rather
    // than hanging or lying.
    let answer = guard(&done_rx, "the in-flight call after its server was killed");
    assert!(
        answer.is_err(),
        "a killed server's call reports the failure: {answer:?}"
    );
    let _ = caller.join();
    eprintln!("the reconnect returned in {reconnect_cost:?} into a 3 s in-flight call");
}

// Verifies: gh #554 - the caller's runtime keeps running while a call blocks.
//
// The behavioural consequence, not the mechanism: two calls on **one
// current-thread** runtime must both make progress. With the blocking work inline,
// the second could not even start until the first finished, so the ordering below
// would be 1-then-2 with the first already returned. `spawn_blocking` is what
// makes it 2-before-1. The total time is reported, never asserted: a slow box must
// not fail this row.
#[test]
fn two_calls_block_concurrently_on_a_current_thread_runtime() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = scratch("concurrent");
    let bridge = Arc::new(lca_mcp::McpBridge::connect_managed(
        engine(&root),
        vec![entry("probe", "probe-mcp-server.py", 1.0)],
    ));
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let started_when_first_done = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");
    let begin = Instant::now();
    let pair = rt.block_on(async {
        let first = {
            let bridge = Arc::clone(&bridge);
            let started = Arc::clone(&started);
            let observed = Arc::clone(&started_when_first_done);
            async move {
                started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let answer = call_async(&bridge).await;
                observed.store(
                    started.load(std::sync::atomic::Ordering::SeqCst),
                    std::sync::atomic::Ordering::SeqCst,
                );
                answer
            }
        };
        let second = {
            let bridge = Arc::clone(&bridge);
            let started = Arc::clone(&started);
            async move {
                // Let the first enter its blocking pool, so the order is a fact
                // rather than a race.
                tokio::time::sleep(Duration::from_millis(50)).await;
                started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                call_async(&bridge).await
            }
        };
        tokio::join!(first, second)
    });
    let total = begin.elapsed();
    assert!(pair.0.is_ok(), "the first call answers: {:?}", pair.0);
    assert!(pair.1.is_ok(), "the second call answers: {:?}", pair.1);
    assert_eq!(
        started_when_first_done.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the second call had started before the first returned, so this runtime \
         was free while the first was blocked (gh #554)"
    );
    // Both calls share one server, so they serialise on its session lock (which
    // is by design, not a defect); the total is reported so a reader can see that
    // it is not "one delay + the other", which is what a stalled runtime would
    // look like... except that it is, for one server. The point of the row is the
    // ordering assertion above.
    eprintln!("two calls on one current-thread runtime took {total:?}");
}

/// One tool call as a future, for the `join!` row.
async fn call_async(bridge: &lca_mcp::McpBridge) -> Result<String, String> {
    bridge
        .execute_tool(&lca_protocol::ToolCall {
            call_id: "call-554".to_string(),
            name: "mcp__probe__probe".to_string(),
            arguments: "{}".to_string(),
            parent_call_id: None,
        })
        .await
        .map(|result| result.content)
        .map_err(|err| format!("{err:?}"))
}

// Verifies: gh #554 - a cancel during the retry backoff is observed promptly.
//
// **This row asserts elapsed time on purpose**, because the claim *is* about
// elapsed time: the backoff is 250 ms then 1 s, and a cancel must be seen inside
// NFR-29's 50 ms budget rather than after the delay. The slice is 25 ms, so the
// bound below is deliberately ~10x the honest figure - it fails only if the sleep
// is uninterruptible again. The measured number is reported.
#[test]
fn a_cancel_during_a_retry_backoff_is_observed_promptly() {
    let mock = common::mock_server();
    let caps = common::sandbox("554-retry", lca_mcp::manifest_grants());
    let bridge = Arc::new(
        lca_mcp::McpBridge::connect_http(caps.clone(), vec![common::remote_server(&mock, "/mcp")])
            .expect("connect"),
    );
    // Two transient failures force a retry, so the second attempt sits inside the
    // backoff. The resource trio retries (idempotent reads); a `tools/call`
    // never does, by contract.
    mock.fail_once("resources/list", 503, None, Vec::new());
    mock.fail_once("resources/list", 503, None, Vec::new());

    let (cancel_tx, cancel_rx) = mpsc::channel();
    let canceller = {
        let caps = caps.clone();
        std::thread::spawn(move || {
            cancel_rx.recv().expect("the row says when to cancel");
            caps.cancel();
        })
    };
    let (answer_tx, answer_rx) = mpsc::channel();
    let caller = {
        let bridge = Arc::clone(&bridge);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("caller runtime");
            let began = Instant::now();
            let answer = rt.block_on(bridge.execute_tool(&lca_protocol::ToolCall {
                call_id: "call-554".to_string(),
                name: lca_mcp::LIST_RESOURCES_TOOL.to_string(),
                arguments: "{}".to_string(),
                parent_call_id: None,
            }));
            let _ = answer_tx.send((answer.is_err(), began.elapsed()));
        })
    };
    // Give the first attempt time to fail and the backoff to start.
    std::thread::sleep(Duration::from_millis(120));
    let cancelled_at = Instant::now();
    cancel_tx.send(()).expect("cancel signal");

    let (failed, elapsed) = guard(&answer_rx, "the cancelled retry");
    canceller.join().ok();
    caps.reset_cancellation();
    let _ = caller.join();
    assert!(failed, "a cancelled retry reports failure, not an answer");
    let observed = cancelled_at.elapsed();
    // 100 ms, not 300: the slice is 25 ms, so the honest figure is a few
    // milliseconds plus scheduling (measured 9-13 ms here) - and 300 ms did not
    // bite, because the *first* backoff is only 250 ms, so an uninterruptible
    // sleep that began before the cancel still finished inside 300 ms of it
    // (measured 163 ms). A bound at 300 ms was a guard that could pass on the
    // defect it exists for; 100 ms fails it (verified by mutation, both ways).
    assert!(
        observed < Duration::from_millis(100),
        "the cancel was observed {observed:?} after it was set; an \
         uninterruptible retry sleep would make the delay elapse first \
         (the call took {elapsed:?})"
    );
    eprintln!(
        "cancel observed {observed:?} into a 250 ms backoff; the call returned after {elapsed:?}"
    );
}
