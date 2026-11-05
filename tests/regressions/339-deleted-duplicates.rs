//! NFR-24 guard for gh #339: the two cleanups whose fix is a **deletion**, so the
//! guard is that the deleted thing cannot come back.
//!
//! Both findings were silent by construction, which is why they are worth a
//! structural row rather than only a behavioural one:
//!
//! - `record_type_name` (`crates/lca-session/src/debug.rs`) carried its own 20-arm copy
//!   of `Record::type_tag`'s vocabulary. Both copies were correct **on the day they were
//!   compared**, so no test could fail; the defect is the *second copy*, which would
//!   have drifted the first time a record type was added to the protocol and the debug
//!   trace kept saying the old word. Deleting it makes the drift a compile error - and
//!   this row makes un-deleting it a test failure.
//! - `Session::new_pending` fell back to `Session::new(empty, empty)`, so an id the
//!   validator rejected would produce a handle whose `log_path()` is the relative
//!   `log.jsonl` - a session log written into whatever directory the process stood in.
//!   The behavioural row lives in `crates/lca-session/tests/lazy.rs`
//!   (`a_pending_session_lands_under_the_sessions_root`); this one pins that the
//!   fallback itself is gone, because a fallback that *happens* to be unreachable today
//!   is still a trap armed for tomorrow.
//!
//! Also the one table the batch deduplicated: `DENY_PROGRAMS` had `"socat"` twice. The
//! behavioural row is beside the table (`every_denied_program_is_listed_once`); this
//! file adds nothing to it deliberately - one guard per defect, not two.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The file's **code**, without its comments.
///
/// The doc comments here narrate the deleted shapes by name (that is the point - the
/// next reader should not have to rediscover them), so a row that searched the raw text
/// would fail on its own explanation. Stripping `//!` and `///` keeps the assertions
/// about what the code does.
fn code(rel: &str) -> String {
    read(rel)
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with("//!") || trimmed.starts_with("///"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The record vocabulary spelled once, in the protocol.
#[test]
fn the_session_debug_trace_does_not_carry_a_second_record_vocabulary() {
    let source = code("crates/lca-session/src/debug.rs");
    assert!(
        source.contains("record.type_tag()"),
        "the trace asks the protocol for the name: {source}"
    );
    // The old copy matched on every variant; a single `Record::` arm means it is back.
    assert!(
        !source.contains("Record::"),
        "no second arm list: the vocabulary lives in `Record::type_tag` only: {source}"
    );
}

/// The pending handle is built under the sessions root, never from an empty path.
#[test]
fn the_pending_session_has_no_empty_path_fallback() {
    let source = code("crates/lca-session/src/lazy.rs");
    assert!(
        !source.contains("Session::new(String::new()"),
        "no empty-id/empty-dir fallback handle: {source}"
    );
    assert!(
        !source.contains("fn new_pending(&self, project_dir: &Path, title: &str) -> Session {\n        self.new_pending_with_id("),
        "and `new_pending` takes the generated id directly, not through the \
         validating path it cannot fail: {source}"
    );
    // The validating path still validates - the fallback's removal must not have
    // removed the refusal with it.
    assert!(
        source.contains("crate::valid_session_id(id)"),
        "the path that takes an id from outside still validates it: {source}"
    );
}

/// **Why** the deleted fallback was a data-location bug, stated as a fact rather than
/// as prose: an empty session directory joins to a *relative* path.
///
/// This is the mechanism behind the behaviour the row in `crates/lca-session/tests/lazy.rs`
/// pins. Without it, "the fallback was wrong" is an assertion about history; with it, a
/// future refactor that reintroduces a bare handle fails here for the reason the fallback
/// was removed rather than for its spelling.
#[test]
fn an_empty_session_directory_logs_into_the_working_directory() {
    let empty = PathBuf::new();
    let log = empty.join("log.jsonl");
    assert_eq!(
        log,
        PathBuf::from("log.jsonl"),
        "an empty directory joins to the bare file name"
    );
    assert!(
        !log.is_absolute(),
        "so an append would create it relative to the process's own directory: {}",
        log.display()
    );
}
