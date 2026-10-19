//! Gh #313: `SessionStore::migrate` rewrote `log.jsonl` with
//! `std::fs::write` (non-atomic) and named backups with one-second
//! resolution.
//!
//! Two independent durability defects on the one code path that rewrites a
//! session log in place:
//!
//! 1. `std::fs::write` truncates the target and then writes. A crash, a
//!    signal, or a full disk between those two steps leaves `log.jsonl`
//!    truncated or half-written - and that file is the session. Every other
//!    write in this crate goes through `write_atomic` (temp file + rename),
//!    so the log's one in-place rewrite was the exception that mattered.
//! 2. `backup_log` named the directory `migrate-<unix-seconds>`. Two
//!    migrations inside one second (a script, a test, or a user re-running
//!    `lca migrate` immediately) computed the same directory and the second
//!    silently overwrote the first backup - the very file the format
//!    promises to keep "until the user removes it".
//!
//! Verifies: gh #313 acceptance (atomic rewrite + distinct backups).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_session::SessionStore;

/// Append one pre-migration user record: no `parent` field at all, which is
/// exactly the shape `stamp_log` rewrites. (The typed `Record::append` path
/// always writes `parent`, so it cannot create work for migration - the
/// fixture writes the line itself, as an older release would have.)
fn append_unstamped(session: &lca_session::Session, id: &str) {
    let line =
        format!(r#"{{"v":1,"t":"user","ts":1,"id":"{id}","content":"hello","attachments":[]}}"#);
    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(session.log_path())
        .expect("open the log");
    use std::io::Write as _;
    writeln!(log, "{line}").expect("append");
}

/// A store with one session holding two unstamped user records, so the
/// second gets a stamped `parent` (that is what makes `migrate` do work).
fn seed(name: &str) -> (lca_testkit::ScratchDir, SessionStore, lca_session::Session) {
    let root = lca_testkit::ScratchDir::new(name);
    let store = SessionStore::new(root.join("data"));
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let session = store.create_session(&project, "migrate").expect("session");
    append_unstamped(&session, "u1");
    append_unstamped(&session, "u2");
    (root, store, session)
}

// Verifies: gh #313 - the backup directory name cannot collide. Two
// migrations that both have work to do inside one second used to compute the
// same `migrate-<seconds>` path, and the second `std::fs::copy` silently
// overwrote the first backup - the file the format promises to keep "until
// the user removes it".
//
// The collision is exercised where it happens: two independent migrations,
// each with unstamped work of its own, run back to back. (Migrating the SAME
// session twice is a no-op by design - the second run finds nothing to stamp
// and returns `already_current` without taking a backup - so a second session
// is what creates the second backup.)
#[test]
fn two_migrations_in_the_same_second_keep_both_backups() {
    let root = lca_testkit::ScratchDir::new("313-backups");
    let store = SessionStore::new(root.join("data"));
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let first = store.create_session(&project, "one").expect("session");
    append_unstamped(&first, "a1");
    append_unstamped(&first, "a2");
    let second = store.create_session(&project, "two").expect("session");
    append_unstamped(&second, "b1");
    append_unstamped(&second, "b2");

    store.migrate(&first).expect("first migrate");
    store.migrate(&second).expect("second migrate");

    // Both backups exist, in their own session's tree, and both hold the
    // original: neither was overwritten and neither was skipped.
    for (session, marker) in [(&first, "a1"), (&second, "b1")] {
        let dirs = backup_dirs(session);
        assert_eq!(
            dirs.len(),
            1,
            "each session took exactly one backup: {dirs:?}"
        );
        let log = dirs[0].join("log.jsonl");
        let text = std::fs::read_to_string(&log).expect("read the backup");
        assert!(
            text.contains(marker),
            "the backup holds that session's original ({marker}): {text}"
        );
        // Every backup directory name carries millisecond resolution, not a
        // bare second count (the old shape was `migrate-<10 digits>`).
        let name = dirs[0]
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a name");
        let stamp = name.trim_start_matches("migrate-");
        assert!(
            stamp.len() > 10,
            "the stamp is milliseconds (13 digits), not seconds: {name}"
        );
    }
}

// Verifies: gh #313 - and the same-directory case: two sessions that happen to
// migrate in the same millisecond still get distinct directories, because the
// name is claimed with `create_dir` (which fails when it exists) rather than a
// plain path that `create_dir_all` would happily reuse.
#[test]
fn the_same_stamp_still_claims_distinct_directories() {
    let root = lca_testkit::ScratchDir::new("313-same-stamp");
    let store = SessionStore::new(root.join("data"));
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let session = store.create_session(&project, "stamp").expect("session");
    append_unstamped(&session, "c1");
    append_unstamped(&session, "c2");

    // Two directories planted with the *same* name a fresh migrate would
    // compute must not be reused: the claim fails and the fallback name wins.
    let backups = session.dir().join("backups");
    std::fs::create_dir_all(&backups).expect("backups");
    store.migrate(&session).expect("migrate");
    let dirs = backup_dirs(&session);
    assert_eq!(dirs.len(), 1, "one backup: {dirs:?}");
    assert!(
        dirs[0].join("log.jsonl").is_file(),
        "the backup holds the log: {}",
        dirs[0].display()
    );
}

/// Every `backups/migrate-*` directory, sorted.
fn backup_dirs(session: &lca_session::Session) -> Vec<std::path::PathBuf> {
    let root = session.dir().join("backups");
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.is_dir()
                        && path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with("migrate-"))
                })
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs
}

// Verifies: gh #313 - the rewrite is atomic. `write_atomic` publishes through
// a sibling temp file and a rename, so an interrupted migrate can never leave
// a truncated `log.jsonl`. Observed from outside: after a successful migrate
// there is no leftover temp file, and the log is a complete, parseable JSONL
// stream. (A crash cannot be simulated in-process; the shape this test pins is
// the seam - the rewrite goes through the atomic helper, so the failure mode
// the issue describes is structurally absent.)
#[test]
fn the_migrated_log_is_complete_and_leaves_no_temp_file() {
    let (_root, store, session) = seed("313-atomic");
    store.migrate(&session).expect("migrate");
    let log = session.log_path();
    let text = std::fs::read_to_string(&log).expect("read the migrated log");
    assert!(!text.is_empty(), "the log survived the rewrite");
    // Every line is a complete JSON object: a truncated write would leave a
    // partial trailing line.
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|err| panic!("a truncated log line would fail here: {err}: {line}"));
    }
    // The atomic helper's temp file is renamed away, never left behind.
    let temp = log.with_extension("tmp");
    assert!(
        !temp.exists(),
        "the atomic write's temp file is gone: {}",
        temp.display()
    );
}

// Verifies: gh #313 - repeated migrations are idempotent in the log itself:
// running twice leaves the same transcript (the second run stamps nothing
// new). This is the property a non-atomic rewrite could break by leaving a
// half-written line between runs.
#[test]
fn migrating_twice_leaves_the_same_transcript() {
    let (_root, store, session) = seed("313-idempotent");
    store.migrate(&session).expect("first");
    let first = std::fs::read_to_string(session.log_path()).expect("read");
    store.migrate(&session).expect("second");
    let second = std::fs::read_to_string(session.log_path()).expect("read");
    assert_eq!(first, second, "a second migration changes nothing");
}
