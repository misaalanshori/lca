wit_bindgen::generate!({
    path: "../../wit",
    world: "hooks-tool-call",
    export_macro_name: "export_hooks_tool_call",
    with: {
        "lca:host/log@0.6.0": generate,
    },
});

use exports::lca::ext::hook_tool_call::{Guest, ToolCall, ToolCallPatch};

pub struct HooksToolCallComponent;

impl Guest for HooksToolCallComponent {
    fn on_tool_call(call: ToolCall) -> ToolCallPatch {
        let patch = crate::mutate_args(&call.arguments);
        ToolCallPatch {
            arguments: patch.arguments,
            block: patch.block,
        }
    }
}

export_hooks_tool_call!(HooksToolCallComponent);
