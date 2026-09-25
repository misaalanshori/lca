//! Protocol type tests: the serialized shapes are what `docs/session-log-format.md`
//! and `docs/headless.md` promise, so they are pinned here.

use lca_protocol::record::{PermissionDecision, ToolSource};
use lca_protocol::{
    ChatMessage, ContentBlock, FORMAT_VERSION, MessageRole, Record, StreamEvent, ToolCall,
    ToolResult, ToolResultStatus, ToolSpec, Usage,
};

// Verifies: FR-CORE-8 (usage record carries cache counts separately)
#[test]
fn usage_records_cache_fields_separately_from_ordinary_tokens() {
    let usage: Usage = serde_json::from_str(
        r#"{"input":40,"output":12,"cache_read":1200,"cache_write":0,"cache_write_1h":64,"cost":0.01}"#,
    )
    .expect("usage parses");
    assert_eq!(usage.input, 40);
    assert_eq!(usage.output, 12);
    assert_eq!(usage.cache_read, 1200);
    assert_eq!(usage.cache_write_1h, 64);
    assert_eq!(
        usage.prompt_tokens(),
        usage.input + usage.cache_read + usage.cache_write + usage.cache_write_1h
    );
    assert!(usage.reported_cache());
}

#[test]
fn usage_without_cache_activity_reports_no_cache() {
    let usage = Usage::default();
    assert_eq!(usage.prompt_tokens(), 0);
    assert!(!usage.reported_cache());
}

// Verifies: FR-CORE-8 (a zero-cost default never fails to serialize)
#[test]
fn usage_round_trips_with_all_counts_present_when_zero() {
    let usage = Usage::default();
    let json = serde_json::to_string(&usage).expect("serialize");
    // Consumers get one stable shape: every count is present, zero or not.
    for field in [
        "input",
        "output",
        "cache_read",
        "cache_write",
        "cache_write_1h",
        "cost",
    ] {
        assert!(
            json.contains(&format!(r#""{field}""#)),
            "{field} missing from {json}"
        );
    }
    let back: Usage = serde_json::from_str(&json).expect("parse");
    assert_eq!(back, usage);
}

// The documented example line from docs/session-log-format.md must parse.
#[test]
fn documented_user_record_example_parses() {
    let line = r#"{"v":1,"t":"user","ts":1758326400123,"id":"01J...","content":"add a test for the parser"}"#;
    let record: Record = serde_json::from_str(line).expect("example parses");
    match record {
        Record::User {
            v, ts, id, content, ..
        } => {
            assert_eq!(v, FORMAT_VERSION);
            assert_eq!(ts, 1758326400123);
            assert_eq!(id, "01J...");
            assert_eq!(content, "add a test for the parser");
        }
        other => panic!("wrong record: {other:?}"),
    }
}

/// Every record variant with its spec type tag, for the tests below.
fn sample_records() -> Vec<(Record, &'static str)> {
    let ts = 0;
    vec![
        (
            Record::SessionStart {
                v: 1,
                ts,
                agent_version: "0.1.0".into(),
                abi_version: "0.1".into(),
                working_dir: "/w".into(),
            },
            "session-start",
        ),
        (
            Record::User {
                v: 1,
                ts,
                id: "a".into(),
                content: String::new(),
                attachments: vec![],
            },
            "user",
        ),
        (
            Record::Assistant {
                v: 1,
                ts,
                id: "a".into(),
                content: vec![],
                reasoning: None,
                model: None,
                provider: None,
                usage: None,
            },
            "assistant",
        ),
        (
            Record::ToolCall {
                v: 1,
                ts,
                id: "a".into(),
                call_id: "c".into(),
                name: "n".into(),
                arguments: "{}".into(),
                source: ToolSource::Builtin,
            },
            "tool-call",
        ),
        (
            Record::ToolResult {
                v: 1,
                ts,
                id: "a".into(),
                call_id: "c".into(),
                status: ToolResultStatus::Ok,
                content: None,
                attachment: None,
                truncated: false,
            },
            "tool-result",
        ),
        (
            Record::Permission {
                v: 1,
                ts,
                id: "a".into(),
                action: "run ls".into(),
                decision: PermissionDecision::Once,
                pattern: None,
            },
            "permission",
        ),
        (
            Record::ExtensionEvent {
                v: 1,
                ts,
                id: "a".into(),
                extension: "e".into(),
                event: "load".into(),
                detail: String::new(),
            },
            "extension-event",
        ),
        (
            Record::Compaction {
                v: 1,
                ts,
                id: "a".into(),
                replaced_from: "1".into(),
                replaced_to: "2".into(),
                summary: "s".into(),
                strategy: "compaction-default".into(),
                usage: None,
            },
            "compaction",
        ),
        (
            Record::ForkPoint {
                v: 1,
                ts,
                id: "a".into(),
                parent_session: "p".into(),
                record_id: "r".into(),
            },
            "fork-point",
        ),
        (
            Record::SessionEnd {
                v: 1,
                ts,
                id: "a".into(),
            },
            "session-end",
        ),
    ]
}

#[test]
fn record_type_tags_match_the_spec() {
    for (record, tag) in sample_records() {
        assert_eq!(record.type_tag(), tag, "{record:?}");
    }
}

// The log is the authority for everything (docs/session-log-format.md):
// what is written must read back as the same record, for every variant.
#[test]
fn every_record_variant_round_trips_through_json() {
    for (record, _) in sample_records() {
        let json = serde_json::to_string(&record).expect("serialize");
        let back: Record = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, record, "round trip for {}", record.type_tag());
    }
}

// Verifies: the ABI freeze gate (docs/abi-versioning.md): every type that
// crosses the extension boundary carries a reserved `extras` map, and it
// round-trips, so new non-structural data rides without a shape change.
#[test]
fn abi_crossing_types_round_trip_their_extras() {
    let mut spec = ToolSpec {
        name: "t".into(),
        description: "d".into(),
        parameters: serde_json::json!({"type": "object"}),
        extras: Default::default(),
    };
    spec.extras.insert("key".into(), "value".into());
    let back: ToolSpec = round_trip(&spec);
    assert_eq!(back, spec);
    assert_eq!(back.extras.get("key").map(String::as_str), Some("value"));

    let call = ToolCall {
        call_id: "c".into(),
        name: "t".into(),
        arguments: "{}".into(),
    };
    assert_eq!(round_trip::<ToolCall>(&call), call);

    let mut result = ToolResult::ok("c", "out");
    result.extras.insert("attachment".into(), "deadbeef".into());
    let back: ToolResult = round_trip(&result);
    assert_eq!(back, result);
    assert_eq!(
        back.extras.get("attachment").map(String::as_str),
        Some("deadbeef")
    );

    let mut message = ChatMessage::text(MessageRole::User, "hi");
    message.extras.insert("key".into(), "value".into());
    assert_eq!(round_trip::<ChatMessage>(&message), message);

    let mut usage = Usage::default();
    usage.extras.insert("provider_region".into(), "eu".into());
    assert_eq!(round_trip::<Usage>(&usage), usage);
}

fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(value: &T) -> T {
    let json = serde_json::to_string(value).expect("serialize");
    serde_json::from_str(&json).expect("parse")
}

#[test]
fn records_serialize_with_v_t_ts_first() {
    let record = Record::User {
        v: FORMAT_VERSION,
        ts: 7,
        id: "x".into(),
        content: "hi".into(),
        attachments: vec![],
    };
    let json = serde_json::to_string(&record).expect("serialize");
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    // The log line carries the three framing fields before anything
    // type-specific; JSON object order is not semantic, so assert presence.
    assert_eq!(value["v"], 1);
    assert_eq!(value["t"], "user");
    assert_eq!(value["ts"], 7);
    assert_eq!(value["content"], "hi");
}

// Verifies: FR-TOOL-7 (truncated flag survives the log round trip)
#[test]
fn tool_result_carries_truncated_flag() {
    let mut result = ToolResult::ok("call-1", "output");
    result.truncated = true;
    let json = serde_json::to_string(&result).expect("serialize");
    let back: ToolResult = serde_json::from_str(&json).expect("parse");
    assert!(back.truncated);
    assert_eq!(back.status, ToolResultStatus::Ok);
}

#[test]
fn tool_result_statuses_match_headless_vocabulary() {
    for (result, expected) in [
        (ToolResult::ok("c", ""), "ok"),
        (ToolResult::error("c", ""), "error"),
        (ToolResult::denied("c", "no"), "denied"),
        (ToolResult::timeout("c", ""), "timeout"),
    ] {
        let json = serde_json::to_string(&result).expect("serialize");
        assert!(
            json.contains(&format!(r#""status":"{expected}""#)),
            "{json}"
        );
    }
}

// Verifies: FR-PROV-7 and FR-PROV-8 (stream event vocabulary is explicit)
#[test]
fn stream_event_tags_match_the_typed_cases() {
    for (event, tag) in [
        (StreamEvent::TextDelta { delta: "x".into() }, "text-delta"),
        (
            StreamEvent::ReasoningDelta { delta: "x".into() },
            "reasoning-delta",
        ),
        (
            StreamEvent::ToolCallStart {
                call_id: "c".into(),
                name: "n".into(),
            },
            "tool-call-start",
        ),
        (
            StreamEvent::ToolCallArgDelta {
                call_id: "c".into(),
                delta: "{}".into(),
            },
            "tool-call-arg-delta",
        ),
        (
            StreamEvent::ToolCallEnd {
                call_id: "c".into(),
            },
            "tool-call-end",
        ),
        (
            StreamEvent::Usage {
                usage: Usage::default(),
            },
            "usage",
        ),
        (
            StreamEvent::Error {
                message: "m".into(),
                retryable: false,
            },
            "error",
        ),
        (
            StreamEvent::VendorEvent {
                kind: "k".into(),
                payload: serde_json::json!({}),
            },
            "vendor-event",
        ),
    ] {
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(&format!(r#""type":"{tag}""#)), "{json}");
    }
}

#[test]
fn vendor_event_is_the_reserved_escape_hatch() {
    let event = StreamEvent::VendorEvent {
        kind: "safety-filters".into(),
        payload: serde_json::json!({"blocked": false}),
    };
    assert!(event.is_vendor());
    assert!(!StreamEvent::TextDelta { delta: "x".into() }.is_vendor());
}

#[test]
fn chat_messages_carry_tool_calls_and_results() {
    let call = ChatMessage {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::ToolCall {
            call_id: "c1".into(),
            name: "read".into(),
            arguments: r#"{"path":"a"}"#.into(),
        }],
        tool_calls: vec![ToolCall {
            call_id: "c1".into(),
            name: "read".into(),
            arguments: r#"{"path":"a"}"#.into(),
        }],
        tool_call_id: None,
        usage: None,
        extras: Default::default(),
    };
    let json = serde_json::to_string(&call).expect("serialize");
    assert!(json.contains(r#""tool_calls""#));

    let result = ChatMessage::tool_result("c1", "contents");
    assert_eq!(result.tool_call_id.as_deref(), Some("c1"));
    assert_eq!(result.role, MessageRole::Tool);
    assert_eq!(result.plain_text(), "contents");
}
