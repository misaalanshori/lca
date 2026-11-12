//! The listing cache rows (FR-SESS-2): `index.json` rebuilt from the
//! directories, newest first, and the labels a picker shows.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use super::common::{scratch, store, user_record};

// A project with no sessions yet must list an empty set, not fail while
// creating the index cache (FR-SESS-2's first-run case).
#[test]
fn listing_a_project_with_no_sessions_is_empty_not_an_error() {
    let store = store("empty-list");
    let project = scratch("empty-list-project");
    let listed = store.list_sessions(&project).expect("empty project lists");
    assert!(listed.is_empty(), "no sessions yet: {listed:?}");
}

// Verifies: FR-SESS-2 (resume lists sessions for the project, newest first)
#[test]
fn lists_sessions_newest_first() {
    let store = store("list");
    let project = scratch("list-project");
    let first = store.create_session(&project, "first").expect("create");
    store
        .append(&first, user_record("r0", "hello"))
        .expect("append");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = store.create_session(&project, "second").expect("create");
    store
        .append(&second, user_record("r1", "hi"))
        .expect("append");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let third = store.create_session(&project, "third").expect("create");
    store
        .append(&third, user_record("r2", "now"))
        .expect("append");

    let listed = store.list_sessions(&project).expect("list");
    let titles: Vec<&str> = listed.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, vec!["third", "second", "first"]);
    let second_entry = listed.iter().find(|s| s.title == "second").expect("second");
    assert_eq!(
        second_entry.message_count, 1,
        "index carries the message count"
    );
    assert_eq!(second_entry.id, second.id());
}

// Verifies: FR-SESS-2 (the listing is a list of sessions worth resuming).
// The interface creates a session at every launch, so a listing that
// includes message-less sessions buries real ones under a wall of
// `0 messages` rows - the manual side-by-side against pi's picker, which
// never shows an empty session, is what surfaced it (2026-10-01). Hidden
// from the list only: `lca resume <id>` still opens one directly.
#[test]
fn sessions_without_messages_are_not_listed() {
    let store = store("hide-empty");
    let project = scratch("hide-empty-project");
    let _abandoned = store
        .create_session(&project, "never used")
        .expect("create");
    let used = store.create_session(&project, "used").expect("create");
    store
        .append(&used, user_record("r1", "hi"))
        .expect("append");

    let listed = store.list_sessions(&project).expect("list");
    let titles: Vec<&str> = listed.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, vec!["used"], "empty sessions stay out: {titles:?}");
}

// The index.json file is a cache: delete it and listing still works
// (docs/session-log-format.md, layout on disk).
#[test]
fn index_is_rebuilt_when_missing() {
    let store = store("index-rebuild");
    let project = scratch("index-rebuild-project");
    let session = store.create_session(&project, "only").expect("create");
    store
        .append(&session, user_record("r1", "hi"))
        .expect("append");
    std::fs::remove_file(store.index_path(&project)).expect("remove index");
    let listed = store.list_sessions(&project).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].message_count, 1);
}

// Verifies: FR-SESS-2 (the listing reflects current content, not a cache
// snapshot from creation). `lca resume` showed `0 messages` for a session
// that had grown, because the index cache is written at creation and only
// rebuilt when another session is created.
#[test]
fn listing_reflects_messages_appended_after_creation() {
    let store = store("list-fresh");
    let project = scratch("list-fresh-project");
    let session = store.create_session(&project, "grows").expect("create");
    store
        .append(&session, user_record("r1", "hi"))
        .expect("append1");
    store
        .append(&session, user_record("r2", "there"))
        .expect("append2");
    let listed = store.list_sessions(&project).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].message_count, 2,
        "the count reflects records appended after creation"
    );
}

// Verifies: FR-SESS-2 (a listing tells its sessions apart). Rows carry the
// title when the session has one and the first prompt while it still has
// the default title - pi's session rows are "name-or-first-message"
// (pi-tui-re/src_re/agent-components/selectors-large.md), and a picker of
// identically-titled rows gave nothing to pick by in the manual drive
// (2026-10-01).
#[test]
fn listing_labels_fall_back_to_the_first_prompt() {
    let store = store("label");
    let project = scratch("label-project");
    let session = store
        .create_session(&project, lca_session::DEFAULT_TITLE)
        .expect("create");
    store
        .append(
            &session,
            user_record("r1", "\x1b[1;31mfix the bug\x1b[0m\nwith the details"),
        )
        .expect("append");

    let listed = store.list_sessions(&project).expect("list");
    assert_eq!(
        listed[0].display_title(),
        "fix the bug with the details",
        "the first prompt, escape sequences stripped, whitespace collapsed"
    );

    store.rename(&session, "renamed by hand").expect("rename");
    let listed = store.list_sessions(&project).expect("list");
    assert_eq!(
        listed[0].display_title(),
        "renamed by hand",
        "a real title wins over the prompt"
    );
}

// The label's bounds: default passthrough, an OSC sequence dropped whole,
// and the 80-column cut with an ellipsis (a picker row is one line).
#[test]
fn row_labels_are_sanitized_and_bounded() {
    use lca_session::{DEFAULT_TITLE, row_label};
    assert_eq!(row_label("renamed", Some("ignored")), "renamed");
    assert_eq!(row_label(DEFAULT_TITLE, None), DEFAULT_TITLE);
    assert_eq!(row_label(DEFAULT_TITLE, Some("a\tb")), "a b");
    assert_eq!(
        row_label(DEFAULT_TITLE, Some("\x1b]0;pasted title\x07hello")),
        "hello",
        "an OSC sequence is dropped whole, not half-stripped into text"
    );
    let long = "x".repeat(200);
    let label = row_label(DEFAULT_TITLE, Some(&long));
    assert_eq!(label.chars().count(), 81, "80 columns plus the ellipsis");
    assert!(label.ends_with('…'), "{label}");
}
