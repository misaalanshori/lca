//! The headless `--json` envelope serializer (testing plan section
//! 13, gh #99): arbitrary turn events through `HeadlessSink` must come
//! out as one valid JSON object per line, each carrying its `type` tag.
//! A malformed envelope breaks every `--json` consumer downstream, so
//! the serializer is attacker-adjacent (event text is model-controlled).
#![no_main]

use libfuzzer_sys::fuzz_target;

use lca_core::TurnSink;
use lca_protocol::{ToolCall, ToolResult, ToolResultStatus, TurnEvent, Usage};

/// The sink's output buffer, shared so the target reads back what the
/// sink wrote.
#[derive(Clone, Default)]
struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Shared {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn text(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    // The first byte picks the event shape; the rest fills its fields.
    // Every string-carrying variant is covered, so model-controlled
    // text (quotes, backslashes, control bytes) always passes through
    // the serializer.
    let (shape, rest) = (data[0] % 10, &data[1..]);
    let event = match shape {
        0 => TurnEvent::TurnStarted,
        1 => TurnEvent::MessageStarted { role: "assistant" },
        2 => TurnEvent::MessageEnded { role: "assistant" },
        3 => TurnEvent::TextDelta(text(rest)),
        4 => TurnEvent::AssistantText(text(rest)),
        5 => TurnEvent::Usage(Usage {
            input: rest.len() as u64,
            output: rest.len() as u64,
            ..Usage::default()
        }),
        6 => TurnEvent::ToolStarted(ToolCall {
            call_id: text(rest),
            name: text(rest),
            arguments: text(rest),
            parent_call_id: None,
        }),
        7 => TurnEvent::ToolFinished(ToolResult {
            call_id: text(rest),
            status: ToolResultStatus::Ok,
            content: text(rest),
            truncated: false,
            exit_code: None,
            images: Vec::new(),
            full_output_path: None,
            nested: Vec::new(),
            extras: Default::default(),
            duration_ms: None,
        }),
        8 => TurnEvent::ToolOutputChunk {
            call_id: text(rest),
            chunk: text(rest),
        },
        _ => TurnEvent::Error {
            message: text(rest),
            class: text(rest),
            retryable: false,
        },
    };
    let shared = Shared::default();
    let mut sink = lca_cli::HeadlessSink::with_writer(true, false, Box::new(shared.clone()));
    sink.on_event(event);
    drop(sink);
    let bytes = shared
        .0
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let body = String::from_utf8_lossy(&bytes);
    assert!(!body.is_empty(), "an event always emits at least one line");
    for line in body.lines() {
        let value: serde_json::Value =
            serde_json::from_str(line).expect("every envelope line is valid JSON");
        assert!(
            value.get("type").and_then(|kind| kind.as_str()).is_some(),
            "every envelope carries its type tag: {line}"
        );
    }
});
