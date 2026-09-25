//! The request-assembly snapshot (testing-plan section "the assembled
//! request"): for a canonical session state - fixed history, fixed
//! prompt, no variation - the exact message list the host would hand a
//! provider's `stream-completion` call is committed to the repository.
//! A change to prompt assembly, tool-call merging, or the record-to-
//! message mapping shows up as a file diff in review instead of as a
//! quiet cache break against a real provider that a fake provider
//! would never notice.
//!
//! Refresh deliberately with `UPDATE_SNAPSHOT=1 cargo test -p lca-core
//! --test assembly_snapshot` and read the diff before committing it.

use lca_core::assemble;
use lca_protocol::{Record, ToolResultStatus, ToolSource};

/// The prompt for the canonical session: ordinary instructions plus a
/// skill line, so the snapshot covers prompt-side content the way a
/// real session assembles it.
const SYSTEM_PROMPT: &str = "\
You are the LCA coding agent. Follow the repository's instructions.

# skill: commit-style
Write commit messages as one intent-bearing sentence.

Rules:
- answer in English
- never invent file paths";

/// The canonical history: a greeting turn, then a tool-using turn with
/// reasoning attached to the answer, plus records that must not reach
/// the wire at all (session markers, a permission decision).
fn canonical_records() -> Vec<Record> {
    vec![
        Record::SessionStart {
            v: 1,
            ts: 1_700_000_000_000,
            agent_version: "lca-1.0.0".into(),
            abi_version: "0.2".into(),
            working_dir: "/workspace/project".into(),
        },
        Record::User {
            v: 1,
            ts: 1_700_000_001_000,
            id: "u-1".into(),
            content: "List the Rust files in src/.".into(),
            attachments: vec![],
        },
        Record::Assistant {
            v: 1,
            ts: 1_700_000_002_000,
            id: "a-1".into(),
            content: vec![],
            reasoning: Some("The user wants a listing; the shell tool fits.".into()),
            model: Some("fixture-model".into()),
            provider: Some("openai-compatible".into()),
            usage: None,
        },
        Record::ToolCall {
            v: 1,
            ts: 1_700_000_002_500,
            id: "t-1".into(),
            call_id: "call-1".into(),
            name: "shell".into(),
            arguments: "{\"command\":[\"ls\",\"src\"]}".into(),
            source: ToolSource::Builtin,
        },
        Record::ToolResult {
            v: 1,
            ts: 1_700_000_003_000,
            id: "r-1".into(),
            call_id: "call-1".into(),
            status: ToolResultStatus::Ok,
            content: Some("main.rs\nlib.rs\n".into()),
            attachment: None,
            truncated: false,
        },
        Record::Assistant {
            v: 1,
            ts: 1_700_000_004_000,
            id: "a-2".into(),
            content: vec![lca_protocol::ContentBlock::Text {
                text: "src/ holds main.rs and lib.rs.".into(),
            }],
            reasoning: None,
            model: Some("fixture-model".into()),
            provider: Some("openai-compatible".into()),
            usage: None,
        },
        Record::Permission {
            v: 1,
            ts: 1_700_000_004_500,
            id: "p-1".into(),
            action: "run ls in /workspace/project".into(),
            decision: lca_protocol::PermissionDecision::Denied,
            pattern: None,
        },
        Record::User {
            v: 1,
            ts: 1_700_000_005_000,
            id: "u-2".into(),
            content: "Thanks.".into(),
            attachments: vec![],
        },
    ]
}

#[test]
fn the_request_assembly_snapshot_never_drifts_silently() {
    let assembled = assemble(&canonical_records(), SYSTEM_PROMPT);
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "messages": assembled.messages,
        "stable_prefix": assembled.stable_prefix,
        "compaction_seen": assembled.compaction_seen,
    }))
    .expect("assembly serializes")
        + "\n";

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join("request-assembly.json");
    if std::env::var_os("UPDATE_SNAPSHOT").is_some() {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, &json).expect("write snapshot");
        return;
    }
    let committed = std::fs::read_to_string(&path).expect(
        "the committed snapshot is missing - run with UPDATE_SNAPSHOT=1 and commit the file",
    );
    assert_eq!(
        committed, json,
        "the assembled request drifted from tests/snapshots/request-assembly.json; \
         review what changed and refresh deliberately with UPDATE_SNAPSHOT=1"
    );
}

// Verifies: FR-CACHE-5/FR-CACHE-6 (the stable prefix ends at the compaction
// summary; a turn after the compaction stays in the dynamic suffix, so the
// new user message does not invalidate the cached prefix). A regression here
// is expensive and invisible in functional tests.
#[test]
fn after_compaction_a_new_turn_stays_outside_the_stable_prefix() {
    fn text(message: &lca_protocol::ChatMessage) -> String {
        message
            .content
            .iter()
            .filter_map(|block| match block {
                lca_protocol::ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    let records = vec![
        Record::User {
            v: 1,
            ts: 1,
            id: "u1".into(),
            content: "old".into(),
            attachments: vec![],
        },
        Record::Assistant {
            v: 1,
            ts: 2,
            id: "a1".into(),
            content: vec![],
            reasoning: None,
            model: None,
            provider: None,
            usage: None,
        },
        Record::Compaction {
            v: 1,
            ts: 3,
            id: "c1".into(),
            replaced_from: "u1".into(),
            replaced_to: "a1".into(),
            summary: "the summary".into(),
            strategy: "compaction-default".into(),
            usage: None,
        },
        Record::User {
            v: 1,
            ts: 4,
            id: "u2".into(),
            content: "new turn".into(),
            attachments: vec![],
        },
    ];
    let assembled = assemble(&records, "sys");
    let summary = assembled
        .messages
        .iter()
        .position(|message| text(message) == "the summary")
        .expect("the summary is on the wire");
    let new_turn = assembled
        .messages
        .iter()
        .position(|message| text(message) == "new turn")
        .expect("the new turn is on the wire");
    assert_eq!(
        assembled.stable_prefix,
        summary + 1,
        "everything through the summary is the stable prefix"
    );
    assert!(
        new_turn >= assembled.stable_prefix,
        "the new turn is in the dynamic suffix, not the cached prefix"
    );
}
