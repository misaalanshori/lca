wit_bindgen::generate!({
    path: "../../wit",
    world: "hooks-tool-result",
    export_macro_name: "export_hooks_tool_result",
    with: {
        "lca:host/log@0.6.0": generate,
    },
});

use exports::lca::ext::hook_tool_result::{Guest, ToolCall, ToolResult, ToolResultPatch};

pub struct HooksToolResultComponent;

impl Guest for HooksToolResultComponent {
    fn on_tool_result(_call: ToolCall, outcome: ToolResult) -> ToolResultPatch {
        ToolResultPatch {
            content: outcome.content.as_deref().and_then(crate::redact_text),
            is_error: None,
        }
    }
}

export_hooks_tool_result!(HooksToolResultComponent);
