wit_bindgen::generate!({
    path: "../../wit",
    world: "hooks",
    export_macro_name: "export_hooks",
    with: {
        "lca:host/ui-dialogs@0.6.0": generate,
    },
});

use exports::lca::ext::hook_attention_required::Guest as AttentionGuest;
use exports::lca::ext::hook_post_tool_use::{
    Guest as PostToolGuest, ToolCall as PostCall, ToolResult as PostResult,
};
use exports::lca::ext::hook_post_turn_end::Guest as PostTurnGuest;
use exports::lca::ext::hook_pre_tool_use::{Action, Guest as PreToolGuest, ToolCall};
use exports::lca::ext::hook_pre_turn::Guest as PreTurnGuest;
use exports::lca::ext::hook_session_close::Guest as CloseGuest;

pub struct HooksComponent;

impl PreTurnGuest for HooksComponent {
    fn on_pre_turn() {}
}

impl PreToolGuest for HooksComponent {
    fn on_pre_tool_use(call: ToolCall) -> Action {
        // One policy source (crate::pre_tool_action), mapped to the
        // WIT variant: the native twin runs the identical decision.
        match crate::pre_tool_action(&call.name) {
            lca_protocol::HookAction::Allow => Action::Allow,
            lca_protocol::HookAction::Deny(reason) => Action::Deny(reason),
            lca_protocol::HookAction::Replace(replacement) => Action::Replace(ToolCall {
                call_id: replacement.call_id,
                name: replacement.name,
                arguments: replacement.arguments,
                extras: Vec::new(),
            }),
        }
    }
}

impl PostToolGuest for HooksComponent {
    fn on_post_tool_use(_call: PostCall, _outcome: PostResult) {}
}

impl PostTurnGuest for HooksComponent {
    fn on_post_turn_end(_status: String) {}
}

impl AttentionGuest for HooksComponent {
    fn on_attention_required(_reason: String) {}
}

impl CloseGuest for HooksComponent {
    fn on_session_close() {}
}

export_hooks!(HooksComponent);
