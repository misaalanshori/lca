//! Conformance extension, `tool` world slice (Phase 2). Later phases add
//! the remaining worlds as the WIT grows.
//!
//! One component covers every host-path case by dispatching on the call's
//! `mode` argument:
//!
//! - `ok` (default): behave normally.
//! - `trap`: panic inside the call (FR-EXT-3's input).
//! - `loop`: spin forever (fuel FR-EXT-4 and epoch FR-CONC-1 input).
//! - `log`: emit one very long log line (FR-EXT-10's input).
//! - `alloc`: allocate far past a small memory ceiling (FR-EXT-5's input).

wit_bindgen::generate!({
    path: "../../wit",
    world: "tool",
    with: {
        "lca:host/log@0.1.0": generate,
    },
});

use exports::lca::ext::execute::{Guest as ExecuteGuest, ToolResult};
use exports::lca::ext::tool_schema::{Guest as SchemaGuest, Schema};
use lca::ext::types::ToolCall;

struct Component;

fn mode(arguments: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get("mode")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "ok".to_string())
}

fn result(call_id: &str, status: &str, content: &str) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        status: status.to_string(),
        content: Some(content.to_string()),
        truncated: false,
        extras: vec![],
    }
}

impl SchemaGuest for Component {
    fn get_schema() -> Schema {
        Schema {
            name: "conformance".to_string(),
            description: "ABI conformance probe: dispatches on the mode argument.".to_string(),
            parameters: r#"{"type":"object","properties":{"mode":{"type":"string"}}}"#.to_string(),
            extras: vec![],
        }
    }
}

impl ExecuteGuest for Component {
    fn run(call: ToolCall) -> ToolResult {
        match mode(&call.arguments).as_str() {
            "trap" => panic!("conformance trap requested"),
            "loop" => loop {
                std::hint::spin_loop();
            },
            "log" => {
                lca::host::log::info(&"x".repeat(50_000));
                result(&call.call_id, "ok", "logged")
            }
            "alloc" => {
                let mut hog: Vec<Vec<u8>> = Vec::new();
                for i in 0..64u64 {
                    let mut block = vec![0u8; 4 * 1024 * 1024];
                    block[0] = i as u8;
                    hog.push(block);
                }
                result(&call.call_id, "ok", "allocated")
            }
            _ => result(&call.call_id, "ok", "conformance ok"),
        }
    }
}

export!(Component);
