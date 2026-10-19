//! Gh #313: `SessionStore::migrate` rewrote `log.jsonl` with
//! `std::fs::write` (truncate-then-write, non-atomic) and named its backup
//! directory with one-second resolution.
//!
//! The log IS the session. Every other write in the crate publishes through
//! `write_atomic` (temp file + rename), so the one place that rewrote the log
//! in place was the exception that mattered: a crash, a signal, or a full disk
//! between truncate and write left a truncated log - and the backup that was
//! supposed to survive it was taken by the same code path.
//!
//! Backups were named `migrate-<unix-seconds>`, so two migrations inside one
//! second computed the same directory and the second `std::fs::copy` silently
//! overwrote the first, contradicting the format's promise that the original
//! survives "until the user removes it".
//!
//! Verifies: gh #313 acceptance (atomic rewrite, distinct backups).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use lca_session::SessionStore;

/// Append one pre-migration user record (no `parent`), which is the shape
/// `stamp_log` rewrites - so migration has work to do.
fn append_unstamped(session: &lca_session::Session, id: &str) {
    use std::io::Write as _;
    let line =
        format!(r#"{{"v":1,"t":"user","ts":1,"id":"{id}","content":"hi","attachments":[]}}"#);
    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(session.log_path())
        .expect("open the log");
    writeln!(log, "{line}").expect("append");
}

/// A session holding two unstamped records.
fn session_ready_to_migrate(
    root: &lca_testkit::ScratchDir,
    name: &str,
) -> (SessionStore, lca_session::Session) {
    let store = SessionStore::new(root.join("data"));
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let session = store.create_session(&project, name).expect("session");
    append_unstamped(&session, "u1");
    append_unstamped(&session, "u2");
    (store, session)
}

fn backup_dirs(session: &lca_session::Session) -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(session.dir().join("backups"))
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs
}

// Verifies: gh #313 - the rewritten log is a complete JSONL stream and the
// atomic write leaves no temp file behind. A truncated write would leave a
// partial trailing line, which is exactly what the failing assertion reports.
#[test]
fn the_migrated_log_is_complete_jsonl_with_no_leftover_temp() {
    let root = lca_testkit::ScratchDir::new("313-reg-atomic");
    let (store, session) = session_ready_to_migrate(&root, "atomic");
    store.migrate(&session).expect("migrate");

    let log = session.log_path();
    let text = std::fs::read_to_string(&log).expect("read the migrated log");
    assert!(!text.trim().is_empty(), "the log survived the rewrite");
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|err| panic!("a truncated line would fail here: {err}: {line}"));
    }
    assert!(
        !log.with_extension("tmp").exists(),
        "the atomic write's temp file was renamed away"
    );
}

// Verifies: gh #313 - the backup directory name carries millisecond
// resolution. The old shape was `migrate-<10-digit seconds>`, which is the
// collision the issue reports.
#[test]
fn the_backup_stamp_is_milliseconds() {
    let root = lca_testkit::ScratchDir::new("313-reg-stamp");
    let (store, session) = session_ready_to_migrate(&root, "stamp");
    store.migrate(&session).expect("migrate");
    let dirs = backup_dirs(&session);
    assert_eq!(dirs.len(), 1, "one backup: {dirs:?}");
    let name = dirs[0]
        .file_name()
        .and_then(|name| name.to_str())
        .expect("a directory name");
    let stamp = name.trim_start_matches("migrate-");
    assert!(
        stamp.len() >= 13,
        "milliseconds, not seconds (got {name:?})"
    );
}

// Verifies: gh #313 - two sessions migrated back to back both keep their
// backup, and each holds its own original (no cross-session overwrite).
#[test]
fn back_to_back_migrations_keep_both_backups() {
    let root = lca_testkit::ScratchDir::new("313-reg-backups");
    let store = SessionStore::new(root.join("data"));
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    let first = store.create_session(&project, "one").expect("session");
    append_unstamped(&first, "a1");
    append_unstamped(&first, "a2");
    let second = store.create_session(&project, "two").expect("session");
    append_unstamped(&second, "b1");
    append_unstamped(&second, "b2");

    store.migrate(&first).expect("first");
    store.migrate(&second).expect("second");

    for (session, marker) in [(&first, "a1"), (&second, "b1")] {
        let dirs = backup_dirs(session);
        assert_eq!(dirs.len(), 1, "one backup each: {dirs:?}");
        let backup = std::fs::read_to_string(dirs[0].join("log.jsonl")).expect("read");
        assert!(
            backup.contains(marker),
            "the backup holds this session's original ({marker})"
        );
    }
}

// Verifies: gh #313 - a migration is idempotent in the log: a second run over
// an already-current session changes nothing (it reports `already_current` and
// takes no backup). A non-atomic rewrite could leave a half-written line that
// a second run then "fixed", which would hide the corruption.
#[test]
fn a_second_migration_changes_nothing() {
    let root = lca_testkit::ScratchDir::new("313-reg-idempotent");
    let (store, session) = session_ready_to_migrate(&root, "idem");
    store.migrate(&session).expect("first");
    let first = std::fs::read_to_string(session.log_path()).expect("read");
    let report = store.migrate(&session).expect("second");
    assert!(report.already_current, "nothing left to stamp");
    let second = std::fs::read_to_string(session.log_path()).expect("read");
    assert_eq!(first, second, "the transcript is unchanged");
    assert_eq!(backup_dirs(&session).len(), 1, "and no extra backup");
}
