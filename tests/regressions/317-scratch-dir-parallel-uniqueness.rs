//! Gh #317: `lca_testkit::ScratchDir::new` keyed its path on the name and the
//! process id only, so every call in one test binary collided.
//!
//! Under `cargo test`, tests in a binary run on parallel threads. Two that
//! happened to pass the same name computed the exact same
//! `<temp>/lca-test-<name>-<pid>`, and the second call's `remove_dir_all`
//! deleted the first one's live directory out from under it. The loser failed
//! for a reason that had nothing to do with what it was testing - the worst
//! kind of flake, because the failure points at innocent code.
//!
//! Nextest's one-process-per-test hides it; the workspace runs nextest, so the
//! bug rarely surfaced. `cargo test` - which the testing plan still documents
//! as the fallback - hit it.
//!
//! Verifies: gh #317 acceptance (unique paths for every call).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

// Verifies: gh #317 - consecutive calls with one name get distinct, live
// directories. This is the direct assertion on the defect: the old code
// returned the same path twice.
#[test]
fn consecutive_calls_with_one_name_get_distinct_live_directories() {
    let first = lca_testkit::ScratchDir::new("317-consecutive");
    let second = lca_testkit::ScratchDir::new("317-consecutive");
    assert_ne!(first.path(), second.path(), "distinct paths");
    assert!(first.path().is_dir(), "the first is still on disk");
    assert!(second.path().is_dir(), "the second is on disk");

    // And the first is usable after the second was created: a file written
    // through the first handle survives.
    let probe = first.join("probe.txt");
    std::fs::write(&probe, b"alive").expect("write through the first handle");
    assert_eq!(std::fs::read(&probe).expect("read"), b"alive");
}

// Verifies: gh #317 - the multi-threaded race the issue describes: many
// threads in one process, one name, every thread's directory alive for the
// whole scope. Holding the guards until the scope ends is what catches a
// clobbering implementation (a `Drop` that removed a sibling's directory would
// show up as a missing path).
#[test]
fn threads_racing_on_one_name_each_keep_their_directory() {
    let dirs: std::sync::Arc<std::sync::Mutex<Vec<lca_testkit::ScratchDir>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for index in 0..12 {
            let dirs = dirs.clone();
            scope.spawn(move || {
                let dir = lca_testkit::ScratchDir::new("317-race");
                // Each thread writes proof of life into its own directory.
                std::fs::write(dir.join("thread.txt"), format!("{index}"))
                    .expect("write into the thread's directory");
                dirs.lock().expect("dirs").push(dir);
            });
        }
    });
    let dirs = dirs.lock().expect("dirs");
    assert_eq!(dirs.len(), 12, "every thread registered its directory");
    let mut paths: Vec<&std::path::Path> = dirs.iter().map(|dir| dir.path()).collect();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), 12, "twelve distinct paths");
    for dir in dirs.iter() {
        assert!(
            dir.join("thread.txt").is_file(),
            "the directory survived its siblings: {}",
            dir.path().display()
        );
    }
}

// Verifies: gh #317 - `scratch_path` inherits the uniqueness, since it is the
// helper 249 call sites actually use.
#[test]
fn the_path_helper_is_unique_too() {
    let first = lca_testkit::scratch_path("317-helper");
    let second = lca_testkit::scratch_path("317-helper");
    assert_ne!(first, second, "the path helper is unique per call too");
    assert!(first.is_dir() && second.is_dir(), "both exist");
}

// Verifies: gh #317 - the DOWNSTREAM consequence the uniqueness change exposed:
// a helper that scratches a directory and a test that scratched the same name
// to find it used to share one path by collision. `extensions/mcp`'s
// `engine(name)` did exactly that, so `scratch("mcp-deny")` after
// `engine("mcp-deny", ..)` only "found" the engine's data directory because
// both calls collided. With unique paths the helper must hand its root back -
// which is the correct shape anyway: a test asserting on a directory should
// receive that directory, not recompute it.
//
// The general form is what this row pins: two calls with one name never alias,
// so no test can accidentally depend on recomputing another call's path.
#[test]
fn a_second_call_never_aliases_the_first() {
    let first = lca_testkit::scratch_path("317-alias");
    std::fs::create_dir_all(first.join("data")).expect("mkdir");
    let second = lca_testkit::scratch_path("317-alias");
    assert_ne!(first, second, "the two paths are distinct");
    assert!(
        !second.join("data").exists(),
        "the second path does not see the first's contents: {}",
        second.display()
    );
    assert!(
        first.join("data").is_dir(),
        "and the first is untouched: {}",
        first.display()
    );
}
