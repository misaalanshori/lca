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

pub struct CommandComponent;

impl SpecGuest for CommandComponent {
    fn get_spec() -> Spec {
        let leaf = crate::command_leaf();
        Spec {
            name: leaf.name,
            hint: leaf.hint,
            completion: leaf.completion,
            extras: Vec::new(),
        }
    }
}

impl InvokeGuest for CommandComponent {
    fn run(argument: String) -> Effect {
        match crate::invoke_command(&argument) {
            lca_protocol::CommandEffect::InsertText(text) => Effect::InsertText(text),
            lca_protocol::CommandEffect::SubmitPrompt(text) => Effect::SubmitPrompt(text),
            lca_protocol::CommandEffect::ShowWidget(text) => Effect::ShowWidget(text),
            // Host-only (the built-in `/attach`); an extension never
            // produces it, so the note is the faithful WIT fallback.
            lca_protocol::CommandEffect::AttachImage { note, .. } => Effect::ShowWidget(note),
            lca_protocol::CommandEffect::None => Effect::None,
        }
    }
}

export_command!(CommandComponent);
