wit_bindgen::generate!({
    path: "../../wit",
    world: "hooks-message",
    export_macro_name: "export_hooks_message",
    with: {
        "lca:host/log@0.6.0": generate,
    },
});

use exports::lca::ext::hook_message_end::{Guest, MessageEndResult};

pub struct HooksMessageComponent;

impl Guest for HooksMessageComponent {
    fn on_message_end(_role: String, text: String) -> MessageEndResult {
        MessageEndResult {
            replacement: crate::redact_text(&text),
        }
    }
}

export_hooks_message!(HooksMessageComponent);
