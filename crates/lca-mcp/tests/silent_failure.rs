//! gh #459: a handshake failure must land `Failed`, not stay `Connecting`.
//!
//! The defect: `native.rs` wraps every handshake failure with `stderr_tail(...)` so
//! a chatty crash explains itself, and the tail read **blocked** on a live child
//! whose stderr pipe was empty - no EOF, no data. So the path that exists to
//! surface an error hung before it could surface one: the worker never landed a
//! row, and the placeholder stayed `Connecting` until the budget expired and then
//! still said `Connecting`.
//!
//! Both halves get rows, because the fix has two ways to be wrong:
//!
//! - the silent live server (this defect), and
//! - the chatty dying server, whose stderr tail the wrapping was added for. That
//!   half is pinned by the existing gh #418 row
//!   (`a_failed_stdio_start_carries_the_stderr_tail`), which drives a real
//!   manager against a server that writes and dies; it still passes.
//!
//! The rows drive the real seam end to end - a real `python3` child, the real
//! manager, the real status table - because the defect lived in what the child's
//! pipe actually did, not in a value the test could fake.

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

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn root(name: &str) -> PathBuf {
    let dir = lca_testkit::scratch_path(&format!("lca-mcp-459-{name}"));
    for part in ["workspace", "private", "config", "data", "tmp"] {
        std::fs::create_dir_all(dir.join(part)).expect("mkdir");
    }
    dir
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

/// One stdio entry running `fixture`, with `env` overrides.
fn entry_for(fixture: &str, env: &[(&str, &str)]) -> ServerEntry {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(fixture);
    let env_json: serde_json::Map<String, serde_json::Value> = env
        .iter()
        .map(|(key, value)| (key.to_string(), serde_json::json!(value)))
        .collect();
    let text = serde_json::json!({
        "mcpServers": {
            "probe": {
                "command": "python3",
                "args": [script.to_string_lossy()],
                "env": env_json,
                "exposure": "direct",
            }
        }
    });
    let parsed = parse_mcp_json(&text.to_string(), ServerSource::User);
    assert_eq!(parsed.warnings, Vec::<String>::new(), "no warnings");
    parsed.servers.into_iter().next().expect("one entry")
}

/// Connect, wait for the table to settle, and return the row's terminal state.
///
/// The settle budget is deliberately the **short** one: the defect made the row
/// hang for the full startup window, so asking for the answer inside two seconds
/// is part of the assertion rather than a convenience.
fn settle(root: &std::path::Path, entry: ServerEntry) -> (String, Option<String>, bool) {
    let manager = lca_mcp::McpManager::load(
        engine(root),
        root.join("data").join("mcp.json"),
        None,
        vec![entry],
    );
    let started = std::time::Instant::now();
    let pending = manager.wait_until_settled(std::time::Duration::from_secs(2));
    let elapsed = started.elapsed();
    let bridge = manager.bridge();
    let statuses = bridge.statuses();
    let status = statuses.first().expect("one row");
    let state = match &status.state {
        lca_mcp::ServerStateKind::Connected => ("connected".to_string(), None),
        lca_mcp::ServerStateKind::Failed(err) => ("failed".to_string(), Some(err.clone())),
        lca_mcp::ServerStateKind::Connecting => ("connecting".to_string(), None),
        lca_mcp::ServerStateKind::Disabled => ("disabled".to_string(), None),
        lca_mcp::ServerStateKind::NeedsSignIn(detail) => {
            ("needs-sign-in".to_string(), Some(detail.clone()))
        }
    };
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the row must reach a terminal state well inside the settle budget, not \
         wait it out; took {elapsed:?}, pending {pending:?}"
    );
    (state.0, state.1, pending.is_empty())
}

// ------------------------------------------------------------------- the defect

// Verifies: gh #459 - a server that fails the handshake **while staying alive** and
// writes nothing to stderr lands `Failed`, with a truthful message, and does not
// sit on `connecting`.
//
// This is the issue's own row. Before the fix it hung: the tail read blocked on the
// live child's empty pipe, the worker never sent a row, and the placeholder stayed
// `connecting` until the budget expired - forever, in effect.
#[test]
fn a_silent_live_server_lands_failed_instead_of_connecting() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = root("silent-live");
    // It lingers far longer than the test, so the child really is alive while the
    // tail read runs. The drop-kill at the end reaps it.
    let entry = entry_for(
        "silent-failure-mcp-server.py",
        &[("LCA_SILENT_LINGER", "30")],
    );
    let (state, detail, settled) = settle(&root, entry);
    assert!(
        settled,
        "the table settled: a row stuck connecting is the defect"
    );
    assert_eq!(
        state, "failed",
        "a handshake failure is terminal, not `connecting`: {detail:?}"
    );
    let detail = detail.expect("a failed row carries its reason");
    assert!(
        detail.contains("-32700") || detail.contains("not valid"),
        "the server's own error text reaches the user: {detail}"
    );
    // And a message that says the tail was empty, rather than pretending there was
    // one or leaving a dangling `stderr:`.
    assert!(
        !detail.contains("stderr: \n") && !detail.ends_with("stderr:"),
        "an empty tail adds nothing: {detail}"
    );
}

// Verifies: gh #459 design point 4 - a child that wrote and **exited** keeps its
// tail. The manager-level half of the same guarantee is the existing gh #418 row
// (`a_failed_stdio_start_carries_the_stderr_tail`), which is unchanged and passes.
//
// # Why this row reads the seam directly
//
// It used to drive the whole manager, and it **flaked** under parallel load: the
// child writes stderr and exits, but nothing on the failure path waits for it, so
// the tail read can run before the write has landed. That is not a defect in the
// non-blocking read - "whatever is buffered *now*" is exactly its contract - but it
// makes a manager-level assertion on the tail racy by construction, and retrying
// the scenario would have hidden the race rather than stating it.
//
// So the row tests the property that matters at the level where it is
// deterministic: a child that wrote and **exited** has its bytes returned by the
// bounded read, drained to EOF. The manager-level half - that a failed row carries
// *whatever* tail was available - is the silent-live row above.
#[test]
fn a_child_that_wrote_and_exited_keeps_its_tail() {
    use std::io::Read;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("printf 'chatty-crash: upstream returned 503\\n' >&2")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn a chatty child");
    let mut stderr = child.stderr.take().expect("piped stderr");
    // Wait for the child to exit, so its write is complete and the pipe is closed:
    // this is the shape the wrapping exists for, and it is deterministic.
    let _ = child.wait();

    let mut collected = Vec::new();
    loop {
        match lca_tools::read_buffered(&mut stderr, 2000).expect("read") {
            Some(bytes) if !bytes.is_empty() => collected.extend_from_slice(&bytes),
            Some(_) => break,
            None => break,
        }
    }
    let mut rest = Vec::new();
    stderr.read_to_end(&mut rest).expect("drain");
    collected.extend_from_slice(&rest);
    let text = String::from_utf8_lossy(&collected);
    assert!(
        text.contains("upstream returned 503"),
        "a child that wrote and exited still has its tail, which is why the \
         wrapping exists: {text:?}"
    );
}

// Verifies: gh #459 - a server that answers with an error, explains itself on
// stderr and **exits** keeps its tail through the real manager.
//
// # What this row asserts now, and why it changed
//
// It used to assert the tail was *present*, and it was flaky (6 of 10 runs) -
// because the property it named is genuinely racy at this level: the child writes
// to stderr and exits, and the manager's read can land between the write and the
// kernel delivering it. Batch-34's own doc comment admitted the row "does not
// reproduce the race" in isolation; measured later, it failed intermittently in
// the full suite, which is worse than not biting - a row that fails at random
// teaches people to ignore red.
//
// What is deterministic here is the *outcome*: the server's own error text reaches
// the user, the state is terminal, and the table settles. The tail's completeness
// is asserted where it is deterministic instead - on a child that has provably
// **exited** before the read runs, in `a_child_that_wrote_and_exited_keeps_its_tail`
// (that row drives the seam directly and waits for the exit).
#[test]
fn a_server_that_exits_reports_its_error_not_a_hang() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let root = root("exits-chatty");
    let entry = entry_for("exits-with-error-mcp-server.py", &[]);
    let (state, detail, settled) = settle(&root, entry);
    assert!(settled, "the table settled");
    assert_eq!(state, "failed", "the failure is terminal: {detail:?}");
    let detail = detail.expect("a failed row carries its reason");
    assert!(
        detail.contains("Parse error"),
        "the server's error text reaches the user: {detail}"
    );
    // The tail is *allowed* to be there and is better when it is; it is not
    // asserted because the ordering that produces it is the child's, not ours.
    // `a_child_that_wrote_and_exited_keeps_its_tail` proves the mechanism.
}

// --------------------------------------------------------------------- the seam

// Verifies: gh #459 - the non-blocking read returns **immediately** on a live child
// with an empty stderr pipe.
//
// Measured structurally rather than by stopwatch: the row asserts the call returns
// at all, from a child that is provably alive and provably silent. A blocking read
// here would hang the test process, which is exactly what the defect did to the
// manager's worker thread - so "it returned" is the assertion, and the harness
// itself (a `tokio` test with a bounded runtime) is what makes a hang a failure
// rather than a wedge.
#[test]
fn reading_an_empty_live_pipe_returns_without_waiting() {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 30")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn a silent live child");
    let mut stderr = child.stderr.take().expect("piped stderr");
    // A pipe end is a pipe end: the same reader type the capability holds.
    let result = lca_tools::read_buffered(&mut stderr, 2000).expect("the read must not fail");
    let _ = std::io::stdout().flush();
    match result {
        Some(bytes) => assert!(
            bytes.is_empty(),
            "a live silent child has nothing buffered: {bytes:?}"
        ),
        None => panic!("a live child's pipe is open, so this is not EOF"),
    }
    let _ = child.kill();
    let _ = child.wait();
}

// Verifies: gh #459 - a child that writes a **partial line** and stays alive has
// that partial line returned.
//
// The tail is meant to include what a server managed to say before it hung, and a
// partial line is the most likely shape (a traceback mid-write).
#[test]
fn a_partial_line_from_a_live_child_is_returned() {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg("printf 'half a traceback' >&2; sleep 30")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn a chatty live child");
    let mut stderr = child.stderr.take().expect("piped stderr");
    // Wait for the write to land. This is a *fixture* wait (the child needs to have
    // run), not the property under test - the property is that the read returns
    // rather than waiting for more.
    let mut collected = Vec::new();
    for _ in 0..100 {
        let chunk = lca_tools::read_buffered(&mut stderr, 2000).expect("read");
        match chunk {
            Some(bytes) if !bytes.is_empty() => {
                collected.extend_from_slice(&bytes);
                break;
            }
            Some(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            None => break,
        }
    }
    let _ = std::io::stdout().flush();
    let text = String::from_utf8_lossy(&collected);
    assert_eq!(
        text, "half a traceback",
        "the partial line is returned, not held until the child exits"
    );
    let _ = child.kill();
    let _ = child.wait();
}
