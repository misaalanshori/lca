//! In-file branch tests (gh #37 phase 2, ADR-0046, FR-SESS-11):
//! `branch-point` navigation keeps one log, reads follow the tip
//! ancestry, and summaries ride the new branch.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_protocol::{FORMAT_VERSION, Record};
use lca_session::{SessionStore, ViewMode};

fn scratch(name: &str) -> std::path::PathBuf {
    lca_testkit::scratch_path(name)
}

fn store(name: &str) -> SessionStore {
    SessionStore::new(scratch(name))
}

fn user_record(id: &str, content: &str) -> Record {
    Record::User {
        v: FORMAT_VERSION,
        ts: 1,
        id: id.to_string(),
        parent: None,
        content: content.to_string(),
        attachments: vec![],
        queue: None,
    }
}

fn ids(records: &[Record]) -> Vec<String> {
    records
        .iter()
        .filter_map(|r| r.id().map(str::to_string))
        .collect()
}

// Verifies: FR-SESS-11 (navigating to an earlier record and
// continuing keeps ONE log: no new session directory, the abandoned
// path excluded from display reads)
#[test]
fn branching_at_a_record_and_resuming_the_branch_keeps_one_log() {
    let root = scratch("branch-one-log");
    let store = SessionStore::new(root.clone());
    let project = scratch("branch-one-log-project");
    let session = store.create_session(&project, "test").expect("create");
    for id in ["a", "b", "c"] {
        store.append(&session, user_record(id, id)).expect("append");
    }
    store.branch_at(&session, "b").expect("branch");
    store
        .append(&session, user_record("d", "d"))
        .expect("resume");

    // One session directory: a fork would have made two.
    fn session_dirs(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.join("log.jsonl").is_file() {
                    found.push(path);
                } else {
                    session_dirs(&path, found);
                }
            }
        }
    }
    let mut dirs = vec![];
    session_dirs(&root, &mut dirs);
    assert_eq!(dirs.len(), 1, "no new session directory: {dirs:?}");

    // The display view walks the live chain only.
    let shown = store
        .resolved(&session, ViewMode::Display)
        .expect("display");
    let shown_ids = ids(&shown.records);
    assert!(shown_ids.contains(&"b".to_string()));
    assert!(shown_ids.contains(&"d".to_string()));
    assert!(
        !shown_ids.contains(&"c".to_string()),
        "the abandoned path leaves the live view: {shown_ids:?}"
    );
    assert!(
        shown_ids.iter().position(|id| id == "b") < shown_ids.iter().position(|id| id == "d"),
        "chain order: {shown_ids:?}"
    );

    // Audit still walks everything.
    let audit = store.resolved(&session, ViewMode::Audit).expect("audit");
    let audit_ids = ids(&audit.records);
    assert!(
        audit_ids.contains(&"c".to_string()),
        "audit keeps all: {audit_ids:?}"
    );
}

// Verifies: FR-SESS-11 (branching at nothing fails loudly)
#[test]
fn branching_at_a_missing_record_fails() {
    let store = store("branch-missing");
    let project = scratch("branch-missing-project");
    let session = store.create_session(&project, "test").expect("create");
    let err = store
        .branch_at(&session, "nope")
        .expect_err("missing target");
    assert!(err.to_string().contains("nope"), "{err}");
}

// Verifies: FR-SESS-11 (records written before linkage read linearly)
#[test]
fn pre_linkage_logs_read_in_log_order() {
    let store = store("branch-linear");
    let project = scratch("branch-linear-project");
    let session = store.create_session(&project, "test").expect("create");
    // Raw lines, bypassing the auto-stamp: exactly what an old log holds.
    let mut log = std::fs::read_to_string(session.log_path()).expect("read");
    for id in ["a", "b"] {
        log.push_str(&serde_json::to_string(&user_record(id, id)).expect("json"));
        log.push('\n');
    }
    std::fs::write(session.log_path(), log).expect("write");
    let shown = store
        .resolved(&session, ViewMode::Display)
        .expect("display");
    let shown_ids = ids(&shown.records);
    assert!(shown_ids.contains(&"a".to_string()) && shown_ids.contains(&"b".to_string()));
}

// Verifies: FR-SESS-11 (a navigation may record the abandoned path,
// and the new branch inherits it)
#[test]
fn branch_summary_records_the_abandoned_path() {
    let store = store("branch-summary");
    let project = scratch("branch-summary-project");
    let session = store.create_session(&project, "test").expect("create");
    for id in ["a", "b", "c"] {
        store.append(&session, user_record(id, id)).expect("append");
    }
    store
        .summarize_branch(&session, "b", "tried X, failed")
        .expect("summarize");

    let audit = store.resolved(&session, ViewMode::Audit).expect("audit");
    let summary = audit
        .records
        .iter()
        .find_map(|record| match record {
            Record::BranchSummary {
                parent,
                from_id,
                summary,
                ..
            } => Some((parent.clone(), from_id.clone(), summary.clone())),
            _ => None,
        })
        .expect("summary recorded");
    assert_eq!(summary.0, Some("b".to_string()), "at the navigation point");
    assert_eq!(
        summary.1,
        Some("c".to_string()),
        "covering the abandoned leaf"
    );
    assert_eq!(summary.2, "tried X, failed");

    // The live chain carries the summary past the jump.
    let shown = store
        .resolved(&session, ViewMode::Display)
        .expect("display");
    let shown_ids = ids(&shown.records);
    assert!(shown_ids.contains(&"b".to_string()));
    assert!(!shown_ids.contains(&"c".to_string()), "{shown_ids:?}");
    assert!(
        audit
            .records
            .iter()
            .any(|r| matches!(r, Record::BranchPoint { .. })),
        "the jump marker persisted"
    );
}

// Verifies: FR-SESS-11 (appends stamp the tip; the first append after
// session-start stays unstamped)
#[test]
fn appends_stamp_the_tip_parent() {
    let store = store("branch-stamp");
    let project = scratch("branch-stamp-project");
    let session = store.create_session(&project, "test").expect("create");
    store.append(&session, user_record("a", "a")).expect("a");
    store.append(&session, user_record("b", "b")).expect("b");
    let raw = store.read(&session).expect("read");
    let parents: Vec<(String, Option<String>)> = raw
        .records
        .iter()
        .filter_map(|r| {
            r.id()
                .map(|id| (id.to_string(), r.parent().map(str::to_string)))
        })
        .collect();
    assert_eq!(
        parents,
        vec![
            ("a".to_string(), None),
            ("b".to_string(), Some("a".to_string())),
        ],
        "{parents:?}"
    );
}

// Verifies: FR-SESS-11 (bookmarks are file-global: a label on an
// abandoned path still resolves)
#[test]
fn labels_survive_branching() {
    let store = store("branch-labels");
    let project = scratch("branch-labels-project");
    let session = store.create_session(&project, "test").expect("create");
    for id in ["a", "b", "c"] {
        store.append(&session, user_record(id, id)).expect("append");
    }
    store
        .set_label(&session, "c", Some("detour"))
        .expect("label");
    store.branch_at(&session, "b").expect("branch");
    assert_eq!(
        store.resolve_label(&session, "detour").expect("resolve"),
        Some("c".to_string())
    );
}

// Verifies: FR-SESS-11 (sweeping never deletes an abandoned branch's
// attachments: the log still holds them)
#[test]
fn gc_keeps_abandoned_branch_attachments() {
    let store = store("branch-gc");
    let project = scratch("branch-gc-project");
    let session = store.create_session(&project, "test").expect("create");
    let dir = session.dir().join("attachments");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let hash = "a".repeat(64);
    std::fs::write(dir.join(&hash), "full text").expect("write");
    store.append(&session, user_record("a", "a")).expect("a");
    store
        .append(
            &session,
            Record::ToolResult {
                v: FORMAT_VERSION,
                ts: 2,
                id: "t".into(),
                parent: None,
                call_id: "c".into(),
                status: lca_protocol::ToolResultStatus::Ok,
                content: Some("truncated".into()),
                attachment: Some(hash.clone()),
                truncated: true,
                exit_code: None,
                nested: Vec::new(),
                full_output_path: None,
            },
        )
        .expect("tool");
    store.branch_at(&session, "a").expect("branch");
    store
        .append(&session, user_record("d", "d"))
        .expect("resume");
    let deleted = store.gc(&session).expect("gc");
    assert!(
        !deleted.contains(&hash),
        "abandoned content survives: {deleted:?}"
    );
    assert!(dir.join(&hash).is_file(), "the file stays");
}

// Verifies: FR-SESS-11 (forking at a branched record inherits its
// chain, not the raw prefix with the abandoned path)
#[test]
fn fork_at_a_branched_record_inherits_its_chain() {
    let store = store("branch-fork");
    let project = scratch("branch-fork-project");
    let session = store.create_session(&project, "test").expect("create");
    for id in ["a", "b", "c"] {
        store.append(&session, user_record(id, id)).expect("append");
    }
    store.branch_at(&session, "b").expect("branch");
    store
        .append(&session, user_record("d", "d"))
        .expect("resume");
    let child = store.fork(&session, "d").expect("fork");
    let shown = store.resolved(&child, ViewMode::Display).expect("display");
    let shown_ids = ids(&shown.records);
    assert!(shown_ids.contains(&"d".to_string()));
    assert!(
        !shown_ids.contains(&"c".to_string()),
        "no abandoned path: {shown_ids:?}"
    );
}

// Verifies: FR-UI-16 (the entry tree rows the `/tree` picker shows:
// record order oldest-first, depth-indented branches, labels, live
// marks, summaries; tool and jump records stay out).
#[test]
fn entry_tree_rows_branches_labels_and_live_marks() {
    let store = store("entry-tree");
    let project = scratch("entry-tree-project");
    let session = store.create_session(&project, "test").expect("create");
    for id in ["a", "b", "c"] {
        let mut record = user_record(id, &format!("message {id}"));
        if let Record::User { content, .. } = &mut record {
            content.push_str("\nsecond line");
        }
        store.append(&session, record).expect("append");
    }
    store.set_label(&session, "b", Some("mark")).expect("label");
    store.branch_at(&session, "a").expect("branch");
    store
        .append(&session, user_record("d", "new path"))
        .expect("d");

    let rows = store.entry_tree(&session).expect("tree");
    let texts: Vec<String> = rows.iter().map(|row| row.text.clone()).collect();
    // Tool/jump/session rows never appear; the abandoned path does
    // (the tree shows every branch, not the live chain).
    assert!(texts.iter().any(|t| t.contains("message a")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("message c")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("new path")), "{texts:?}");
    assert_eq!(
        rows.len(),
        4,
        "three users plus the branch child: {texts:?}"
    );

    let by_id: std::collections::HashMap<&str, &lca_session::EntryRow> =
        rows.iter().map(|row| (row.id.as_str(), row)).collect();
    assert_eq!(by_id["b"].label.as_deref(), Some("mark"));
    assert_eq!(by_id["a"].depth, 0);
    assert_eq!(by_id["d"].depth, 1, "the branch child indents");
    assert!(by_id["d"].live, "the live chain marks");
    assert!(by_id["a"].live);
    assert!(!by_id["c"].live, "the abandoned path unmarks");
    assert!(!texts.iter().any(|t| t.contains('\n')), "one line per row");
}
