use super::*;
use lca_protocol::FORMAT_VERSION;

fn user(id: &str, content: &str) -> Record {
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

fn assistant(id: &str, text: &str) -> Record {
    Record::Assistant {
        v: FORMAT_VERSION,
        ts: 2,
        id: id.to_string(),
        parent: None,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        reasoning: None,
        reasoning_signature: None,
        provider_thinking_level: None,
        model: None,
        provider: None,
        usage: None,
    }
}

fn texts(messages: &[ChatMessage]) -> Vec<(&MessageRole, String)> {
    messages
        .iter()
        .map(|m| {
            (
                &m.role,
                m.content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect(),
            )
        })
        .collect()
}

// A custom-message injects as a user message carrying its
// provenance in extras; display=false still reaches the model
// (display governs rendering, never visibility).
#[test]
fn a_custom_message_injects_as_a_user_message() {
    let records = vec![
        user("u1", "hi"),
        Record::CustomMessage {
            v: FORMAT_VERSION,
            ts: 2,
            id: "m1".into(),
            parent: None,
            custom_type: "my-extension".into(),
            content: "Injected context...".into(),
            display: false,
            details: None,
        },
    ];
    let assembled = assemble(&records, "sys");
    let texts = texts(&assembled.messages);
    assert_eq!(texts.len(), 3);
    assert_eq!(texts[2].1, "Injected context...");
    let injected = &assembled.messages[2];
    assert_eq!(injected.role, MessageRole::User);
    assert_eq!(
        injected.extras.get("custom-type").map(String::as_str),
        Some("my-extension")
    );
    assert_eq!(
        injected.extras.get("display").map(String::as_str),
        Some("false")
    );
}

// A null edit omits the target; a string edit replaces its text.
#[test]
fn context_edits_omit_and_replace() {
    let records = vec![
        user("u1", "forgettable"),
        user("u2", "original"),
        Record::ContextEdit {
            v: FORMAT_VERSION,
            ts: 3,
            id: "e1".into(),
            parent: None,
            target_id: "u1".into(),
            replacement: None,
        },
        Record::ContextEdit {
            v: FORMAT_VERSION,
            ts: 4,
            id: "e2".into(),
            parent: None,
            target_id: "u2".into(),
            replacement: Some("revised".into()),
        },
    ];
    let assembled = assemble(&records, "sys");
    let texts = texts(&assembled.messages);
    assert_eq!(texts.len(), 2, "system plus the revised message: {texts:?}");
    assert_eq!(texts[1].1, "revised");
}

// The latest edit on a target wins; an assistant replacement keeps
// the tool linkage the providers require.
#[test]
fn the_latest_edit_wins_and_keeps_tool_linkage() {
    let call = lca_protocol::ToolCall {
        call_id: "call_1".into(),
        name: "shell".into(),
        arguments: "{}".into(),
        parent_call_id: None,
    };
    let records = vec![
        assistant("a1", "working"),
        Record::ToolCall {
            v: FORMAT_VERSION,
            ts: 3,
            id: "c1".into(),
            parent: None,
            call_id: call.call_id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            source: lca_protocol::ToolSource::Builtin,
        },
        Record::ToolResult {
            v: FORMAT_VERSION,
            ts: 4,
            id: "r1".into(),
            parent: None,
            call_id: call.call_id.clone(),
            status: lca_protocol::ToolResultStatus::Ok,
            content: Some("ok".into()),
            attachment: None,
            truncated: false,
            duration_ms: None,
            exit_code: None,
            nested: Vec::new(),
            full_output_path: None,
            images: Vec::new(),
        },
        Record::ContextEdit {
            v: FORMAT_VERSION,
            ts: 5,
            id: "e1".into(),
            parent: None,
            target_id: "a1".into(),
            replacement: Some("first".into()),
        },
        Record::ContextEdit {
            v: FORMAT_VERSION,
            ts: 6,
            id: "e2".into(),
            parent: None,
            target_id: "a1".into(),
            replacement: Some("second".into()),
        },
    ];
    let assembled = assemble(&records, "sys");
    let assistant = assembled
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Assistant)
        .expect("the assistant message survives its edit");
    let text: String = assistant
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "second", "the latest edit wins");
    assert_eq!(assistant.tool_calls.len(), 1, "tool linkage survives");
    assert!(
        assembled
            .messages
            .iter()
            .any(|m| m.role == MessageRole::Tool)
    );
}

// Usage, labels, session names, extension state, and the switch
// records change nothing about the assembled content.
#[test]
fn bookkeeping_records_alter_no_content() {
    let records = vec![
        user("u1", "hi"),
        Record::ModelChange {
            v: FORMAT_VERSION,
            ts: 2,
            id: "c1".into(),
            parent: None,
            from: None,
            to: "m".into(),
            provider: "p".into(),
            profile: None,
        },
        Record::ThinkingLevelChange {
            v: FORMAT_VERSION,
            ts: 3,
            id: "t1".into(),
            parent: None,
            level: "high".into(),
        },
        Record::Usage {
            v: FORMAT_VERSION,
            ts: 4,
            id: "g1".into(),
            parent: None,
            kind: "cache_warm".into(),
            provider: None,
            model: None,
            usage: Usage::default(),
        },
        Record::Label {
            v: FORMAT_VERSION,
            ts: 5,
            id: "l1".into(),
            parent: None,
            target_id: "u1".into(),
            label: Some("checkpoint-1".into()),
        },
        Record::SessionInfo {
            v: FORMAT_VERSION,
            ts: 6,
            id: "s1".into(),
            parent: None,
            name: "Refactor auth module".into(),
        },
        Record::Custom {
            v: FORMAT_VERSION,
            ts: 7,
            id: "x1".into(),
            parent: None,
            custom_type: "my-extension".into(),
            data: serde_json::json!({"count": 42}),
        },
    ];
    let assembled = assemble(&records, "sys");
    let texts = texts(&assembled.messages);
    assert_eq!(
        texts,
        vec![
            (&MessageRole::System, "sys".to_string()),
            (&MessageRole::User, "hi".to_string()),
        ]
    );
}
