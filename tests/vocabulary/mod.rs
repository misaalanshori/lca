//! Reserved-string vocabulary coverage contract (gh #108, QA-022):
//! record `t` values, `ModelInfo.extras` keys, `fs` scope names, and
//! `net` patterns are stringly typed by deliberate design (additive
//! evolution, `docs/abi-versioning.md`), so the validators and the doc
//! tables are the only coherence mechanism. Each test below asserts
//! every documented value validates and every undocumented value
//! refuses; a rename or typo on either side goes red here.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use lca_permissions::{FsMode, ScopeGrant, ScopeViolationKind, parse_net_pattern};
use lca_protocol::record::{PermissionDecision, ToolSource};
use lca_protocol::{
    IMAGE_RESIZE_EXTRA, IMAGE_VISION_EXTRA, PROMPT_CACHE_EXTRA, Record, THINKING_LEVELS_EXTRA,
    ToolResultStatus, Usage,
};

/// One of every record the writer can emit: the documented `t`
/// vocabulary (`docs/session-log-format.md`), mirroring the sample
/// records in `lca-protocol/tests/records.rs`.
fn every_record() -> Vec<Record> {
    let ts = 0;
    vec![
        Record::SessionStart {
            v: 1,
            ts,
            agent_version: "0.1.0".into(),
            abi_version: "0.1".into(),
            working_dir: "/w".into(),
        },
        Record::User {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            content: String::new(),
            attachments: vec![],
            queue: None,
        },
        Record::Assistant {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            content: vec![],
            reasoning: None,
            reasoning_signature: None,
            provider_thinking_level: None,
            model: None,
            provider: None,
            usage: None,
        },
        Record::ToolCall {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            call_id: "c".into(),
            name: "n".into(),
            arguments: "{}".into(),
            source: ToolSource::Builtin,
        },
        Record::ToolResult {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            call_id: "c".into(),
            status: ToolResultStatus::Ok,
            content: None,
            attachment: None,
            truncated: false,
            exit_code: None,
            nested: Vec::new(),
            full_output_path: None,
            images: Vec::new(),
        },
        Record::Permission {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            action: "run ls".into(),
            decision: PermissionDecision::Once,
            pattern: None,
        },
        Record::ExtensionEvent {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            extension: "e".into(),
            event: "load".into(),
            detail: String::new(),
        },
        Record::Compaction {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            replaced_from: "1".into(),
            replaced_to: "2".into(),
            first_kept_id: String::new(),
            read_files: Vec::new(),
            modified_files: Vec::new(),
            system_prompt: None,
            summary: "s".into(),
            strategy: "compaction-default".into(),
            usage: None,
        },
        Record::ForkPoint {
            v: 1,
            ts,
            id: "a".into(),
            parent_session: "p".into(),
            record_id: "r".into(),
        },
        Record::ModelChange {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            from: None,
            to: "m".into(),
            provider: "p".into(),
            profile: None,
        },
        Record::ThinkingLevelChange {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            level: "high".into(),
        },
        Record::Usage {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            kind: "cache_warm".into(),
            provider: Some("p".into()),
            model: Some("m".into()),
            usage: Usage::default(),
        },
        Record::Label {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            target_id: "r".into(),
            label: Some("checkpoint-1".into()),
        },
        Record::SessionInfo {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            name: "Refactor auth module".into(),
        },
        Record::Custom {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            custom_type: "my-extension".into(),
            data: serde_json::json!({"count": 42}),
        },
        Record::CustomMessage {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            custom_type: "my-extension".into(),
            content: "Injected context...".into(),
            display: true,
            details: None,
        },
        Record::ContextEdit {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
            target_id: "r".into(),
            replacement: None,
        },
        Record::SessionEnd {
            v: 1,
            ts,
            id: "a".into(),
            parent: None,
        },
        Record::BranchPoint {
            v: 1,
            ts,
            id: "b".into(),
            target_id: "a".into(),
        },
        Record::BranchSummary {
            v: 1,
            ts,
            id: "c".into(),
            parent: Some("a".into()),
            from_id: Some("z".into()),
            summary: "tried X".into(),
        },
    ]
}

// Verifies: gh #108 - what the writer emits, the reader knows: every
// `t` tag must sit in the reader's known-types list (a malformed line
// of a known type stops the reader as corruption; an unknown type is
// skipped), and every emitted record must read back as itself.
#[test]
fn every_emitted_record_tag_is_known_to_the_reader() {
    let known = lca_session::known_record_types();
    assert_eq!(
        known.len(),
        every_record().len(),
        "known list tracks the enum"
    );
    for record in every_record() {
        let tag = record.type_tag();
        assert!(
            known.contains(&tag),
            "the reader knows `{tag}` (docs/session-log-format.md)"
        );
        let json = serde_json::to_string(&record).expect("serialize");
        let back: Record = serde_json::from_str(&json).expect("parse");
        assert_eq!(back.type_tag(), tag, "round trip for {tag}");
    }
}

// Verifies: gh #108 - the documented `fs` scope vocabulary
// (`docs/capabilities.md`) parses in both modes, and anything outside
// it refuses as an unknown scope rather than resolving somewhere.
#[test]
fn every_documented_scope_name_parses() {
    for name in ["workspace", "private", "home-config", "temp"] {
        for mode in [FsMode::Read, FsMode::ReadWrite] {
            let grant = ScopeGrant::parse(name, mode).expect("documented scope parses");
            assert_eq!(grant.scope, name);
            assert_eq!(grant.mode, mode);
        }
    }
    for name in [
        "",
        "sessions",
        "WORKSPACE",
        "workspace/",
        "home_config",
        "root",
    ] {
        assert_eq!(
            ScopeGrant::parse(name, FsMode::Read),
            Err(ScopeViolationKind::UnknownScope),
            "`{name}` is not a scope"
        );
    }
}

// Verifies: gh #108 - the `ModelInfo.extras` keys a guest writes and
// the host reads are the documented wire strings; a typo on either
// side crosses the WASM boundary silently, so the constants pin them.
#[test]
fn documented_extras_keys_match_their_constants() {
    assert_eq!(IMAGE_VISION_EXTRA, "image.vision");
    assert_eq!(IMAGE_RESIZE_EXTRA, "image.resize");
    assert_eq!(PROMPT_CACHE_EXTRA, "prompt_cache");
    assert_eq!(THINKING_LEVELS_EXTRA, "thinking_levels");
    assert_eq!(lca_protocol::QUOTA_GROUPS_EXTRA, "quota-groups");
    assert_eq!(lca_protocol::QUOTA_PLAN_EXTRA, "quota-plan");
    assert_eq!(lca_protocol::QUOTA_ERROR_EXTRA, "quota-error");
}

// Verifies: gh #108 - the documented `net` pattern shapes
// (`docs/capabilities.md`) parse, and the documented refusals (bare
// wildcard, wildcard with a port, non-hostnames) stay refused.
#[test]
fn net_patterns_accept_the_documented_shapes_and_refuse_the_rest() {
    for pattern in [
        "api.example.com",
        "build.example.com:8443",
        "*.example-cdn.com",
        "localhost",
    ] {
        assert!(parse_net_pattern(pattern).is_ok(), "`{pattern}` parses");
    }
    assert!(
        parse_net_pattern("*.example.com")
            .expect("wildcard parses")
            .matches_host("a.b.example.com")
    );
    assert!(
        !parse_net_pattern("api.example.com")
            .expect("exact parses")
            .matches_host("other.example.com")
    );
    for pattern in [
        "",
        "*",
        "*.example.com:443",
        "http://example.com",
        "workspace",
        "*.",
    ] {
        assert!(parse_net_pattern(pattern).is_err(), "`{pattern}` refuses");
    }
}
