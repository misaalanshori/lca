//! Gh #569: a blocked pipe operation must not hold the handle table.
//!
//! The shape the issue names, and the one that made it a hang rather than a
//! nuisance: `process_read_stdout` took the `handles` mutex, looked the entry up,
//! and then **blocked in the pipe read with the table still locked**. A stdout
//! read of a silent child blocks until the child writes or closes, so the table
//! was held for the child's whole lifetime - and `process_kill` needs that table.
//! Cancellation, a timeout and `Drop` shutdown all reach for `process_kill`, and
//! `lca-mcp` drives exactly this: a reader thread in `read_some`, the bridge's
//! `Drop` calling `process_kill`. Reader waits for output, killer waits for the
//! lock, child waits for stdin. Nothing moves.
//!
//! The same root covered stdin: `process_write_stdin` held the table across its
//! blocking write, so a child that never drains its stdin stalled every other
//! handle operation - including the kill that would have stopped it.
//!
//! These rows are behavioural, not structural: no lock introspection, no timing
//! threshold on the *assertion*. The only clock here is a hang guard, and its
//! failure message says which side stalled. Measured before the fix, through the
//! same shape: `process_kill` had not returned after three seconds.
//!
//! Verifies: FR-CONC-3 (a cancelled turn stops what is running), gh #569.

#![cfg(unix)] // real pipes and `/bin/kill`: the same rows on Windows need a console
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lca_permissions::{
    Decision, FsMode, GrantStore, PermissionPrompt, ProposalDiff, ScopeGrant, ScopeRoots,
};
use lca_protocol::CapabilityError;
use lca_tools::{Capabilities, CapabilityGrants};

struct Allow;

impl PermissionPrompt for Allow {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Always
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        true
    }
}

fn engine(name: &str) -> (Arc<Capabilities>, std::path::PathBuf) {
    let root = lca_testkit::scratch_path(name);
    std::fs::create_dir_all(root.join("project")).expect("mkdir");
    let grants = CapabilityGrants {
        process: true,
        // `process_spawn` resolves the working directory through the `fs` scope
        // resolver, so the grant set is "process plus a workspace scope" - what a
        // real spawning extension carries.
        fs_declared: true,
        fs: vec![ScopeGrant {
            scope: "workspace",
            mode: FsMode::ReadWrite,
        }],
        ..CapabilityGrants::default()
    };
    let cap = Capabilities::new(
        name,
        grants,
        ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    );
    (Arc::new(cap), root)
}

/// Is the child's process group gone? `kill(pid, 0)` via `/bin/kill` is a
/// signal-free existence check; the row below asserts the tree died, not just
/// that the kill returned.
fn tree_is_gone(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", "--", &format!("-{pid}")])
        .status()
        .map(|status| !status.success())
        .unwrap_or(true)
}

/// The hang guard. Not an assertion about speed: it only fires when the row
/// would otherwise hang, and it says which of the two sides stalled.
fn wait_for_kill(rx: &mpsc::Receiver<()>, whose_read: &str) -> bool {
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(()) => true,
        Err(_) => {
            panic!(
                "the kill never returned within 10 s while {whose_read}; that is \
                 gh #569's deadlock: the blocked pipe operation is holding the \
                 handle table `process_kill` needs (or the kill itself is stuck)"
            );
        }
    }
}

/// Verifies: gh #569 - a kill reaches a child whose stdout read is blocked.
///
/// The row: `cat` reads its stdin and writes nothing, so `process_read_stdout`
/// blocks with no lock released under the old code. A handshake channel says the
/// reader is *in* the call - not a sleep - and then the kill runs from another
/// thread. Before the fix this never returned.
#[test]
fn a_kill_returns_while_a_stdout_read_is_blocked() {
    // The child records its own pid, which is also its process-group id
    // (`process_group(0)`), so the row can assert the *tree* died rather than only
    // that a call returned. Reading it from a file the child wrote is deterministic;
    // pattern-matching another process would race every other test on this box.
    let (cap, root) = engine("569-read-pid");
    let pidfile = root.join("pid");
    let handle = cap
        .process_spawn(
            "sh",
            &["-c".into(), format!("echo $$ > {}; cat", pidfile.display())],
            "workspace",
        )
        .expect("spawn sh+cat");
    let pid = {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(pid) = std::fs::read_to_string(&pidfile)
                .map_err(|err| err.to_string())
                .and_then(|text| text.trim().parse::<u32>().map_err(|err| err.to_string()))
            {
                break pid;
            }
            assert!(
                Instant::now() < deadline,
                "the child never recorded its pid within 10 s"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    let (reader_started_tx, reader_started_rx) = mpsc::channel();
    let reader = {
        let cap = Arc::clone(&cap);
        std::thread::spawn(move || {
            reader_started_tx.send(()).expect("handshake");
            let read = cap.process_read_stdout(handle, 4096);
            // After the kill the pipe closes: `None` is EOF, and it is the answer
            // the blocked read was always going to get once its child died.
            read.map(|chunk| chunk.map(|bytes| bytes.len()))
        })
    };
    // The handshake says the reader thread is about to enter the call; the
    // `process_kill` below is then issued against a read that is genuinely in it,
    // because `cat` writes nothing until its stdin closes.
    reader_started_rx.recv().expect("reader started");

    let (kill_done_tx, kill_done_rx) = mpsc::channel();
    let begin = Instant::now();
    {
        let cap = Arc::clone(&cap);
        std::thread::spawn(move || {
            let kill = cap.process_kill(handle);
            assert!(kill.is_ok(), "the kill succeeds: {kill:?}");
            let _ = kill_done_tx.send(());
        });
    }
    assert!(
        wait_for_kill(&kill_done_rx, "a stdout read of a silent child was blocked"),
        "unreachable"
    );
    // The kill returning is the requirement; the clock is only reported, so the
    // next reader can see how far from the guard it was.
    let kill_cost = begin.elapsed();

    let outcome = reader.join().expect("the reader thread");
    // EOF *or* the broken-pipe error are both honest answers to "your child died
    // mid-read"; a successful chunk would mean the read never blocked at all.
    assert!(
        matches!(outcome, Ok(None) | Ok(Some(0)) | Err(_)),
        "the blocked read ends with the handle: {outcome:?}"
    );
    assert!(
        tree_is_gone(pid),
        "the child's process group is gone after the kill (pid {pid})"
    );
    assert!(
        matches!(
            cap.process_read_stdout(handle, 4096),
            Err(CapabilityError::NotFound(_))
        ),
        "the kill released the handle"
    );
    eprintln!("the kill returned {kill_cost:?} into a blocked stdout read");
}

/// Verifies: gh #569 - a kill returns while a stdin write is blocked.
///
/// The second shape the issue names: a child that never drains its stdin fills
/// the pipe buffer, and `write_all` blocks. Under the old code the table was held
/// for that too, so an unrelated kill could not be issued either.
#[test]
fn a_kill_returns_while_a_stdin_write_is_blocked() {
    let (cap, _root) = engine("569-write-blocked");
    // `sleep` never reads stdin, so the 64 KiB pipe buffer fills and the write
    // stalls. `cat` would read it instead; that is the point of choosing sleep.
    let handle = cap
        .process_spawn("sleep", &["30".into()], "workspace")
        .expect("spawn sleep");
    let big = vec![b'x'; 512 * 1024];

    let (writer_started_tx, writer_started_rx) = mpsc::channel();
    let writer = {
        let cap = Arc::clone(&cap);
        let big = big.clone();
        std::thread::spawn(move || {
            writer_started_tx.send(()).expect("handshake");
            cap.process_write_stdin(handle, &big)
        })
    };
    writer_started_rx.recv().expect("writer started");
    // Give the write time to fill the buffer and actually block. This is a *setup*
    // wait, not an assertion: a faster machine reaching it sooner only makes the
    // test more decisive, and the guard below is what bounds everything.
    std::thread::sleep(Duration::from_millis(300));

    let (kill_done_tx, kill_done_rx) = mpsc::channel();
    let begin = Instant::now();
    {
        let cap = Arc::clone(&cap);
        std::thread::spawn(move || {
            let kill = cap.process_kill(handle);
            assert!(kill.is_ok(), "the kill succeeds: {kill:?}");
            let _ = kill_done_tx.send(());
        });
    }
    assert!(
        wait_for_kill(
            &kill_done_rx,
            "a stdin write to a non-reading child was blocked"
        ),
        "unreachable"
    );
    let kill_cost = begin.elapsed();

    let outcome = writer.join().expect("the writer thread");
    // The child is dead; a broken pipe is the honest report of that, and an
    // accepted write would mean the buffer had room - the write never blocked.
    match outcome {
        Ok(written) => assert_eq!(written as usize, big.len()),
        Err(CapabilityError::Io(_)) => {}
        Err(other) => panic!("a killed child's stdin write reports i/o or success: {other:?}"),
    }
    eprintln!("the kill returned {kill_cost:?} into a blocked stdin write");
    assert!(matches!(
        cap.process_write_stdin(handle, b"after"),
        Err(CapabilityError::NotFound(_))
    ));
}

/// Verifies: gh #569 - the pipe still delivers what it read, in order, and a
/// concurrent kill does not corrupt or duplicate it.
///
/// The seatbelt for the take/return dance: putting the pipe back must not lose the
/// bytes the caller already has, and bytes read *after* a kill must not be
/// resurrected into a handle nobody owns.
#[test]
fn bytes_read_before_and_after_a_kill_are_delivered_once() {
    let (cap, _root) = engine("569-bytes");
    let handle = cap
        .process_spawn(
            "sh",
            &["-c".into(), "printf 'one'; sleep 0.15; printf 'two'".into()],
            "workspace",
        )
        .expect("spawn");

    let mut first = Vec::new();
    while let Some(chunk) = cap.process_read_stdout(handle, 4096).expect("read") {
        first.extend_from_slice(&chunk);
        if first.len() >= 3 {
            break;
        }
    }
    assert_eq!(
        first, b"one",
        "the first read returns the first bytes, in order"
    );

    // A second read is what a caller does next; the pipe went back after the first,
    // so this one must find it again rather than report it missing.
    let mut rest = Vec::new();
    while let Some(chunk) = cap.process_read_stdout(handle, 4096).expect("second read") {
        if chunk.is_empty() {
            break;
        }
        rest.extend_from_slice(&chunk);
        if rest.len() >= 3 {
            break;
        }
    }
    assert_eq!(rest, b"two", "and the rest after the pause");
    // Both reads came from the same pipe and nothing was duplicated.
    assert_eq!(
        format!(
            "{}{}",
            String::from_utf8_lossy(&first),
            String::from_utf8_lossy(&rest)
        ),
        "onetwo"
    );

    // The trailing kill is best-effort by contract (the child already exited and
    // closed its pipes): NotFound or a successful tree kill are both fine.
    let _ = cap.process_kill(handle);
    assert!(
        matches!(
            cap.process_read_stdout(handle, 4096),
            Err(CapabilityError::NotFound(_))
        ),
        "after the kill, the handle is gone rather than holding a resurrected pipe"
    );
}

/// Verifies: gh #569 - a handle of another kind is refused *without* being
/// consumed, and no pipe from it is taken.
///
/// The rule predates this change and must survive the take/return split: the type
/// check happens before anything is taken out of the entry.
#[test]
fn a_wrong_type_handle_is_refused_without_taking_anything() {
    let root = lca_testkit::scratch_path("569-wrong-type");
    std::fs::create_dir_all(root.join("project")).expect("mkdir");
    let grants = CapabilityGrants {
        process: true,
        pty: true,
        fs_declared: true,
        fs: vec![ScopeGrant {
            scope: "workspace",
            mode: FsMode::ReadWrite,
        }],
        ..CapabilityGrants::default()
    };
    let cap = Capabilities::new(
        "569-wrong-type",
        grants,
        ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Allow)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    );
    let pty = cap
        .pty_spawn("sh", &["-c".into(), "exit 3".into()], "workspace", 24, 80)
        .expect("pty spawn");
    // One type alias keeps the array readable; clippy's type_complexity rule is
    // right that the inline spelling is noise in a row this short.
    type ReadOutcome = Result<Option<Vec<u8>>, CapabilityError>;
    let reads: [(&str, ReadOutcome); 3] = [
        ("stdout", cap.process_read_stdout(pty, 64)),
        ("stderr", cap.process_read_stderr(pty, 64)),
        ("buffered", cap.process_read_stderr_buffered(pty, 64)),
    ];
    for (label, result) in reads {
        match result {
            Err(CapabilityError::Invalid(message)) => assert!(
                message.contains("not a process"),
                "the {label} refusal names the type mismatch: {message}"
            ),
            other => panic!("a pty handle is not a process handle for {label}: {other:?}"),
        }
    }
    // And the write path, whose Ok type is a count rather than bytes.
    match cap.process_write_stdin(pty, b"x") {
        Err(CapabilityError::Invalid(message)) => assert!(
            message.contains("not a process"),
            "the stdin refusal names the type mismatch: {message}"
        ),
        other => panic!("a pty handle is not a process handle for stdin: {other:?}"),
    }
    // The PTY is untouched and still waitable through its own API.
    assert_eq!(cap.pty_wait(pty).expect("pty wait"), 3);
}
