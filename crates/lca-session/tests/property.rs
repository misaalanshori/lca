//! Property-based invariants for the session log (testing plan section 8):
//! round-tripping any sequence of valid records, and truncation at any byte
//! offset always producing the longest valid prefix rather than an error.

use lca_protocol::{ContentBlock, FORMAT_VERSION, Record};
use lca_session::SessionStore;
use proptest::prelude::*;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lca-session-prop-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn arb_text() -> impl Strategy<Value = String> {
    // Any UTF-8, including quotes, newlines, control characters, and
    // backslashes: none of them may break framing.
    any::<String>()
}

fn arb_id(n: u32) -> String {
    format!("id-{n:06}")
}

fn arb_record(n: u32) -> impl Strategy<Value = Record> {
    let ts = (n as u64).wrapping_mul(1000);
    prop_oneof![
        arb_text().prop_map(move |content| Record::User {
            v: FORMAT_VERSION,
            ts,
            id: arb_id(n),
            content,
            attachments: vec![],
        }),
        (arb_text(), arb_text()).prop_map(move |(text, reasoning)| Record::Assistant {
            v: FORMAT_VERSION,
            ts,
            id: arb_id(n),
            content: vec![ContentBlock::Text { text }],
            reasoning: Some(reasoning),
            model: Some("model".into()),
            provider: Some("provider".into()),
            usage: None,
        }),
        (arb_text(), arb_text()).prop_map(move |(name, args)| Record::ToolCall {
            v: FORMAT_VERSION,
            ts,
            id: arb_id(n),
            call_id: arb_id(n),
            name,
            arguments: args,
            source: lca_session::ToolSource::Builtin,
        }),
        (arb_text(), any::<bool>()).prop_map(move |(content, truncated)| Record::ToolResult {
            v: FORMAT_VERSION,
            ts,
            id: arb_id(n),
            call_id: arb_id(n),
            status: lca_session::ToolResultStatus::Ok,
            content: Some(content),
            attachment: None,
            truncated,
        }),
        arb_text().prop_map(move |detail| Record::ExtensionEvent {
            v: FORMAT_VERSION,
            ts,
            id: arb_id(n),
            extension: "ext".into(),
            event: "load".into(),
            detail,
        }),
    ]
}

fn arb_log() -> impl Strategy<Value = Vec<Record>> {
    proptest::collection::vec(arb_record(1), 0..12)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    // Any sequence of valid records writes and reads back identically.
    #[test]
    fn session_log_round_trips(records in arb_log()) {
        let dir = scratch("roundtrip");
        let store = SessionStore::new(dir.clone());
        let project = dir.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let session = store.create_session(&project, "prop").expect("create");
        for record in &records {
            store.append(&session, record.clone()).expect("append");
        }
        let read = store.read(&session).expect("read");
        prop_assert!(!read.truncated);
        let mut expected = vec![store.raw_start(&session).expect("start")];
        expected.extend(records);
        prop_assert_eq!(read.records, expected);
        std::fs::remove_dir_all(&dir).ok();
    }

    // Truncating the log at any byte offset yields a valid prefix of the
    // original sequence, never a failure: the recovery behavior of
    // docs/session-log-format.md.
    #[test]
    fn truncation_at_any_offset_yields_the_valid_prefix(offset in 0usize..4096) {
        let dir = scratch("truncate");
        let store = SessionStore::new(dir.clone());
        let project = dir.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let session = store.create_session(&project, "prop").expect("create");
        for n in 0..20u32 {
            store.append(&session, lca_protocol::Record::User {
                v: FORMAT_VERSION,
                ts: n as u64,
                id: format!("id-{n:04}"),
                content: format!("content {n} with \"quotes\" and \\backslash\\"),
                attachments: vec![],
            }).expect("append");
        }
        let full = std::fs::read(session.log_path()).expect("read log");
        // The untruncated log defines the exact expected sequence.
        let expected: Vec<_> = store.read(&session).expect("read").records;

        let cut = offset.min(full.len());
        std::fs::write(session.log_path(), &full[..cut]).expect("truncate");

        let read = store.read(&session).expect("read must never fail");
        // Truncation only ever loses records from the end: the survivors
        // must equal the original prefix record for record.
        prop_assert!(read.records.len() <= expected.len());
        for (loaded, original) in read.records.iter().zip(expected.iter()) {
            prop_assert_eq!(loaded, original);
        }
        // A cut landing exactly after a closing brace but before its
        // newline still yields one valid extra record.
        let lines_before = full[..cut].iter().filter(|&&b| b == b'\n').count();
        prop_assert!(read.records.len() <= lines_before + 1);
        if cut < full.len() && !read.truncated {
            // Nothing was reported lost, so every newline-terminated line
            // plus at most one newline-free valid record was consumed.
            prop_assert!(read.records.len() >= lines_before);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}

// Verifies: the testing plan's compaction-range invariant - a compaction's
// declared replaced range, applied to any generated session history, never
// hides a record outside that range and always hides the ones inside it.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn compaction_never_hides_a_record_outside_its_range(
        count in 2usize..10,
        start in 0usize..10,
        len in 1usize..10,
    ) {
        use lca_session::ViewMode;
        let dir = scratch("compaction-range");
        let store = SessionStore::new(dir.clone());
        let project = dir.join("project");
        std::fs::create_dir_all(&project).expect("mkdir");
        let session = store.create_session(&project, "prop").expect("create");
        let ids: Vec<String> = (0..count).map(|n| format!("r{n}")).collect();
        for (n, id) in ids.iter().enumerate() {
            store.append(&session, Record::User {
                v: FORMAT_VERSION,
                ts: n as u64,
                id: id.clone(),
                content: format!("message {n}"),
                attachments: vec![],
            }).expect("append");
        }
        let start = start.min(count - 1);
        let end = (start + len - 1).min(count - 1);
        store.append(&session, Record::Compaction {
            v: FORMAT_VERSION,
            ts: 0,
            id: "c0".into(),
            replaced_from: ids[start].clone(),
            replaced_to: ids[end].clone(),
            summary: "summary".into(),
            strategy: "test".into(),
            usage: None,
        }).expect("append compaction");

        let view = store.read_with(&session, ViewMode::Display).expect("read");
        let present: std::collections::BTreeSet<String> = view
            .records
            .iter()
            .filter_map(|r| r.id().map(str::to_string))
            .collect();
        for (n, id) in ids.iter().enumerate() {
            if n < start || n > end {
                prop_assert!(present.contains(id), "record {id} outside [{start},{end}] must survive");
            } else {
                prop_assert!(!present.contains(id), "record {id} inside [{start},{end}] is hidden");
            }
        }
        prop_assert!(present.contains("c0"), "the compaction record itself survives");
        std::fs::remove_dir_all(&dir).ok();
    }
}
