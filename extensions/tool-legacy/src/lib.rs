//! The pre-catalog single-tool shape (gh #77's old-guest compat):
//! one `get-schema` plus `run`, no `tool-catalog` world. The host
//! wraps it as a `direct` tool with no namespace, exactly the shape
//! every tool had before suites.

#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path: "../../wit",
    world: "tool",
    export_macro_name: "export_legacy_tool",
    with: {
        "lca:host/log@0.6.0": generate,
        "lca:host/fs@0.6.0": generate,
        "lca:host/process@0.6.0": generate,
        "lca:host/pty@0.6.0": generate,
        "lca:host/ui-dialogs@0.6.0": generate,
        "lca:host/tools@0.6.0": generate,
        "lca:host/resources@0.6.0": generate,
        "lca:host/state@0.6.0": generate,
    },
});

#[cfg(target_arch = "wasm32")]
use exports::lca::ext::execute::{Guest as ExecuteTrait, ToolResult as WasmResult};
#[cfg(target_arch = "wasm32")]
use exports::lca::ext::tool_schema::{Guest as SchemaGuest, Schema};
#[cfg(target_arch = "wasm32")]
use lca::ext::types::ToolCall;

#[cfg(target_arch = "wasm32")]
pub struct LegacyTool;

#[cfg(target_arch = "wasm32")]
impl SchemaGuest for LegacyTool {
    fn get_schema() -> Schema {
        Schema {
            name: "legacy-echo".to_string(),
            description: "Echoes its input (the pre-catalog shape).".to_string(),
            parameters: r#"{"type":"object","properties":{"text":{"type":"string"}}}"#.to_string(),
            extras: Vec::new(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl ExecuteTrait for LegacyTool {
    fn run(call: ToolCall) -> WasmResult {
        WasmResult {
            call_id: call.call_id,
            status: "ok".to_string(),
            content: Some(format!("legacy-echo: {}", call.arguments)),
            truncated: false,
            extras: Vec::new(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
export_legacy_tool!(LegacyTool);
