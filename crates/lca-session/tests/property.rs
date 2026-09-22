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
