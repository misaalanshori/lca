//! NFR-24 guard for gh #547: cancellation is event-driven, not a poll thread.
//!
//! ## The defect
//!
//! `Agent::run_turn_with` spawned one OS thread per turn that slept 1 ms and
//! checked `cancel.is_cancelled()`, then blocked the async fn on `watcher.join()`
//! when the turn ended. Three costs, all real:
//!
//! 1. **A wakeup per millisecond for the whole turn.** A 60 s stream paid 60,000
//!    wakeups to observe nothing.
//! 2. **Windows timer quanta.** `sleep(1ms)` rounds up to the ~15.6 ms system
//!    tick, so the observer could miss the cancel for a full quantum - a third of
//!    NFR-29's 50 ms budget, inside a bound `docs/platform-notes.md` already
//!    records as needing three fresh-engine attempts under load.
//! 3. **A runtime thread blocked at every turn end** by `join()` inside an
//!    `async fn`.
//!
//! ## What this pins
//!
//! The mechanisms, not the code shape: the callback registry fires on the
//! cancelling thread and the turn uses it. The behavioural rows live where the
//! behaviour can be driven - `crates/lca-tools/src/cancel.rs`'s own tests
//! (already-cancelled fires immediately, a dropped listener is silent, one
//! panicking listener does not stop the rest, a hundred registrations leave
//! nothing behind) and `crates/lca-core/tests/turn_boundary.rs`
//! (`a_cancel_interrupts_the_extension_before_cancel_returns` reads the engine's
//! flag on the line after `cancel()`, which a poller cannot satisfy on any
//! platform, and `a_cancel_after_a_finished_turn_does_not_reach_the_extension`).
//!
//! The one structural claim is the issue's own: **the 1 ms poll thread is gone**,
//! and it is gone for every crate, not just `lca-core`. A wall-clock assertion
//! would reintroduce the load sensitivity this repo keeps removing, so the guard
//! reads the source for the two shapes the defect had - the sleep-and-poll loop
//! and the turn-end `join()`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use std::path::Path;

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|err| panic!("read {rel}: {err}"))
}

/// `crates/**/src/*.rs`, production code only.
fn production_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // `tests/` and `target/` are not the product; a test may sleep
                // however it likes.
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name != "tests" && name != "target" {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            // Test code lives under `tests/`, but also in `src` as `*_tests.rs`
            // and `tests.rs` (the repo's house split for rows that need private
            // access). A test may sleep a millisecond; the product may not.
            let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if file.contains("test") {
                continue;
            }
            let rel = path
                .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| path.display().to_string());
            if let Ok(body) = std::fs::read_to_string(&path) {
                out.push((rel, body));
            }
        }
    }
    out
}

// Verifies: gh #547 - no production file spins a 1 ms polling loop.
//
// The exact literal the defect used (`thread::sleep(Duration::from_millis(1))`)
// is what this forbids, because a cancellation poller at that period is the shape
// the issue is about. A legitimate 1 ms sleep elsewhere - a keyboard repeat, a
// render tick - would have to be justified at its site; none exists today, so
// this is a clean line rather than a rule with carve-outs.
#[test]
fn no_production_path_sleeps_a_millisecond_to_poll_a_flag() {
    let mut offenders = Vec::new();
    for (path, body) in production_sources() {
        // Only production: a `#[cfg(test)]` module inside src is still a test.
        let prod = body
            .split("#[cfg(test)]")
            .next()
            .unwrap_or(&body)
            .to_string();
        for (index, line) in prod.lines().enumerate() {
            let compact = line.split_whitespace().collect::<String>();
            let one_ms = compact.contains("from_millis(1))") || compact.contains("from_millis(1),");
            if compact.contains("thread::sleep(") && one_ms {
                offenders.push(format!("{path}:{}: {}", index + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a 1 ms sleep is a poll thread's period, and gh #547 removed the poll \
         thread (cancellation now runs on the cancelling thread); found:\n{}",
        offenders.join("\n")
    );
}

// Verifies: gh #547 - the turn no longer spawns a watcher thread, and no longer
// joins one inside the async fn.
//
// Two independent statements about `run_turn_with`, because the shapes could
// come back separately: a thread spawn (the poller), and a `join()` at turn end
// (the blocking wait inside `async fn`). Reading the function body rather than
// the whole file matters - `lca-core` may legitimately spawn threads elsewhere.
#[test]
fn the_turn_does_not_spawn_or_join_a_cancellation_watcher() {
    let source = read("crates/lca-core/src/lib.rs");
    let start = source
        .find("pub async fn run_turn_queued(")
        .expect("`run_turn_queued` exists");
    let body = &source[start..];
    let end = body.find("\n    fn fail(").expect("the next method starts");
    let body = &body[..end];

    assert!(
        !body.contains("thread::spawn"),
        "the per-turn watcher thread is back in `run_turn_queued` (gh #547): \
         cancellation is a callback on the flag, not a poller"
    );
    assert!(
        !body.contains(".join()"),
        "the turn joins a thread again (gh #547): `join()` inside an `async fn` \
         blocks a runtime thread at every turn end"
    );
    assert!(
        body.contains("on_cancel("),
        "the turn registers its interrupt callback on the flag: {body}"
    );
}

// Verifies: gh #547 - the callback is registered for the whole body, not a
// local that could be dropped early or never held.
//
// `let _interrupt_guard = ...` is what keeps it alive to the end of the function;
// a bare `flag.on_cancel(...);` would drop the guard on the same line and the
// turn would be uncancellable. That is a one-character difference with a total
// behaviour change, so it is worth a guard of its own.
#[test]
fn the_turn_binds_the_listener_for_its_whole_body() {
    let source = read("crates/lca-core/src/lib.rs");
    let start = source
        .find("pub async fn run_turn_queued(")
        .expect("`run_turn_queued` exists");
    let body = &source[start..];
    let end = body.find("\n    fn fail(").expect("the next method starts");
    let body = &body[..end];
    assert!(
        body.contains("let _interrupt_guard =") && body.contains("cancel.on_cancel("),
        "the guard is bound to a name that lives to the end of the turn: {body}"
    );
    // And it is bound *before* the body runs, so a cancel from any thread while
    // the turn is in flight is already registered.
    let guard = body.find("let _interrupt_guard =").expect("the binding");
    let body_call = body.find(".turn_body(").expect("the turn body runs");
    assert!(
        guard < body_call,
        "the listener is registered before the turn body starts, or a cancel \
         could arrive at a turn that is already running and find nobody"
    );
}

// Verifies: gh #547 - `cancel()` delivers the callbacks itself, on the caller's
// thread, and the registry is keyed so a drop removes only its own entry.
//
// The mechanism is asserted where it lives, because the risk here is a
// "simplification" back to a condvar/waiter: a waiter only helps an async
// caller, and the whole reason the poller existed is that a synchronous WASM
// call cannot await.
#[test]
fn the_flag_runs_its_listeners_inside_cancel() {
    let source = read("crates/lca-tools/src/cancel.rs");
    let start = source
        .find("    pub fn cancel(&self) {")
        .expect("`CancelFlag::cancel` exists");
    let body = &source[start..];
    let end = body
        .find("\n    pub fn is_cancelled")
        .expect("the next method");
    let body = &body[..end];

    assert!(
        body.contains("notify_waiters()"),
        "the async waiter still wakes: {body}"
    );
    assert!(
        body.contains("catch_unwind"),
        "a panicking callback does not stop the rest: {body}"
    );
    assert!(
        body.contains("std::mem::take("),
        "the callbacks are taken, so a second `cancel()` cannot fire them again \
         and a callback that cancels again cannot deadlock on the registry: {body}"
    );
    // The drain happens outside the lock: taking the vec, then running it.
    let take = body.find("std::mem::take(").expect("the take");
    let run = body.find("catch_unwind").expect("the run");
    assert!(
        take < run,
        "the registry is emptied before any callback runs, so callbacks are \
         outside the lock"
    );

    // Keyed removal, not pointer identity: two boxed zero-sized closures can
    // share an address, so identity would unregister the wrong one.
    assert!(
        source.contains("next_key: AtomicU64") && source.contains("next_key.fetch_add(1"),
        "each registration carries a monotonic key"
    );
    assert!(
        source.contains("retain(|(key, _)| *key != self.key)"),
        "and a listener's drop removes exactly its own key"
    );
}

// Verifies: gh #547 - the sync and async attachment twins now agree on the
// create race, and the async one orders its write before it reports the hash.
//
// Two statements because the second is what the fixtures caught: `tokio::fs`
// writes on a blocking thread and its `Drop` only schedules the close, so a
// caller could be handed a hash for a file that still read as empty - and the
// sync twin, seeing an existing file, would then skip writing it. The
// behavioural rows are in `crates/lca-core/tests/attachments.rs`
// (`concurrent_stores_of_the_same_bytes_all_succeed_with_one_file`,
// `a_pre_existing_identical_attachment_is_not_replaced`,
// `the_sync_and_async_stores_agree_on_the_same_file`); this pins that neither
// twin can quietly lose its `AlreadyExists` arm again.
#[test]
fn both_attachment_writers_treat_a_create_race_as_success() {
    let source = read("crates/lca-core/src/assemble.rs");
    let sync_start = source
        .find("fn write_attachment(")
        .expect("the sync writer exists");
    let sync_body = &source[sync_start..];
    let sync_end = sync_body.find("\n}\n").expect("the function ends");
    let sync_body = &sync_body[..sync_end];
    assert!(
        sync_body.contains("ErrorKind::AlreadyExists"),
        "the sync writer ignores a create race, the way its twin does: {sync_body}"
    );

    let async_start = source
        .find("pub async fn store_image_bytes_async(")
        .expect("the async twin exists");
    let async_body = &source[async_start..];
    let async_end = async_body.find("\n}\n").expect("the function ends");
    let async_body = &async_body[..async_end];
    assert!(
        async_body.contains("ErrorKind::AlreadyExists"),
        "the async twin keeps its `AlreadyExists` arm: {async_body}"
    );
    assert!(
        async_body.contains("file.flush()"),
        "and it flushes before reporting a hash, because `tokio::fs` closes \
         asynchronously: {async_body}"
    );
}
