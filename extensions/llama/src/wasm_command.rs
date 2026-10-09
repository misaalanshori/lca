//! WASM command delivery: the spec and a graceful decline. The
//! `command` world imports no `net` (link-time refusal by design), so
//! the sandboxed guest cannot reach the server — it says so in text,
//! and the native twin does the work.

wit_bindgen::generate!({
    path: "../../wit",
    world: "command",
    export_macro_name: "export_command",
    with: {
        "lca:host/ui-dialogs@0.6.0": generate,
    },
});

use exports::lca::ext::command_spec::{Guest as SpecGuest, Spec};
use exports::lca::ext::invoke::{Effect, Guest as InvokeGuest};

pub struct LlamaCommand;

impl SpecGuest for LlamaCommand {
    fn get_spec() -> Spec {
        let leaf = crate::command_spec();
        Spec {
            name: leaf.name,
            hint: leaf.hint,
            completion: leaf.completion,
            extras: Vec::new(),
        }
    }
}

impl InvokeGuest for LlamaCommand {
    fn run(_argument: String) -> Effect {
        Effect::ShowWidget(
            "/llama needs the native delivery: the sandboxed command world cannot reach the local server"
                .to_string(),
        )
    }
}

export_command!(LlamaCommand);
