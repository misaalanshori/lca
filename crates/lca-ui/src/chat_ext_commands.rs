//! Extension-owned command dispatch: the shared tail of host and
//! async paths, plus prompt-template expansion. Split from
//! `chat_commands.rs` for the workspace file ceiling (gate 11).
//! Behaviour unchanged.

use super::chat::Chat;
use crate::Action;
use lca_protocol::CommandEffect;

impl Chat {
    pub(crate) fn dispatch_extension_command(&mut self, name: &str, argument: &str) -> Action {
        match (self.world.options.invoke_command)(name, argument) {
            CommandEffect::ShowWidget(text) => {
                self.say(crate::state::sanitize_block(&text));
            }
            CommandEffect::AttachImage {
                media_type,
                bytes,
                note,
            } => {
                let info = lca_tui::widgets::image::ImageInfo::new(&media_type, &bytes);
                self.transcript.push_image(info, bytes);
                self.say(crate::state::sanitize_block(&note));
            }
            CommandEffect::InsertText(text) => {
                self.editor.insert_str(&crate::state::sanitize_block(&text));
            }
            CommandEffect::SubmitPrompt(text) => {
                self.transcript.push_user(text.clone());
                self.submitted_queue = None;
                self.submitted = Some(text);
                return Action::Submit;
            }
            CommandEffect::None => {}
        }
        Action::Continue
    }

    pub(crate) fn expand_template(&mut self, name: &str, argument: &str) -> Option<String> {
        let templates = self.world.options.hooks.prompt_templates.clone()?;
        let template = templates().into_iter().find(|t| t.name == name)?;
        Some(crate::state::sanitize_block(&lca_tools::prompts::expand(
            &template, argument,
        )))
    }
}
