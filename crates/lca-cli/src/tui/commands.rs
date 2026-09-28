//! The interface's options and built-in command dispatch (S1). The
//! `/model`/`/attach`/`/compact`/`/settings`/`/session`/identity slots were
//! one closure capturing `run`'s locals; they are a method on [`Ui`] now.

use std::sync::Arc;

use lca_config::ColorMode;
use lca_protocol::CommandEffect;
use lca_ui::UiOptions;

use super::BUILTIN_SLOTS;
use super::Ui;
use super::display::model_effect_on;

impl Ui {
    /// The interface's options (S1): the static inputs plus the closure the
    /// interface calls for every slash command.
    pub(super) fn options(self: &Arc<Self>) -> UiOptions {
        let ui = self.clone();
        let invoke_command: lca_ui::CommandInvoker =
            Arc::new(move |name, argument| ui.dispatch_command(name, argument));
        UiOptions {
            model_label: self.label_cell.clone(),
            context_window: self.context_window_cell.clone(),
            thinking: self.thinking_cell.clone(),
            theme: self.config.ui_theme().unwrap_or("auto").to_string(),
            theme_dir: lca_ui::theme::themes_dir(&crate::config_dir().join("lca")),
            themes: lca_ui::theme::theme_names(&lca_ui::theme::themes_dir(
                &crate::config_dir().join("lca"),
            )),
            initial_lines: self.initial_lines.clone(),
            models: self
                .provider
                .list_models()
                .iter()
                .map(|model| model.id.clone())
                .collect(),
            plain: self.config.ui_color() == ColorMode::Never,
            invoke_command,
            render_regions: self.render_regions.clone(),
            ui_events: self.ui_events.clone(),
            update_notice: Some(self.update_notice.clone()),
            login: Some(self.login_options()),
            pick_login: Some(self.login_pick()),
            complete_login: Some(self.login_complete()),
            confirm_login_grant: Some(self.login_confirm()),
            hooks: self.hooks(),
            fullscreen: std::fs::read_to_string(crate::config_dir().join("ui.json"))
                .map(|s| !s.contains("false"))
                .unwrap_or(true),
            slash_commands: self.slash_commands(),
            workspace: self.cwd.clone(),
        }
    }

    /// The completion list: interface-level commands first, then the
    /// built-in slots, then every extension command (sanitized).
    fn slash_commands(&self) -> Vec<String> {
        let mut names: Vec<String> = BUILTIN_SLOTS
            .iter()
            .map(|name| format!("/{name}"))
            .collect();
        // Interface-level commands: `/help` lists everything (answered
        // by the interface itself, so it works with no extension), and
        // `/exit`/`/quit` leave. They lead the list so completion shows
        // them first.
        names.insert(0, "/help".to_string());
        names.insert(1, "/exit".to_string());
        names.insert(2, "/hotkeys".to_string());
        names.insert(3, "/fullscreen".to_string());
        names.insert(4, "/theme".to_string());
        names.insert(5, "/tree".to_string());
        names.insert(6, "/fork".to_string());
        names.insert(7, "/thinking".to_string());
        names.insert(8, "/resume".to_string());
        names.insert(9, "/settings".to_string());
        names.insert(10, "/grants".to_string());
        names.extend(
            self.registry
                .command_names()
                .into_iter()
                .map(|name| format!("/{}", lca_ui::sanitize_text(&name))),
        );
        names
    }

    /// One built-in slash command (the slots the host itself fills, then the
    /// registry's dispatch).
    fn dispatch_command(&self, name: &str, argument: &str) -> CommandEffect {
        match name {
            "attach" => self.command_attach(argument),
            "model" => self.command_model(argument),
            "compact" => {
                match lca_core::compact_now(
                    self.store.clone(),
                    self.session(),
                    self.agent_config.extensions.clone(),
                    self.agent_config.completion_backend.clone(),
                ) {
                    Ok(summary) => CommandEffect::ShowWidget(format!("compacted: {summary}")),
                    Err(detail) => {
                        CommandEffect::ShowWidget(format!("nothing was compacted: {detail}"))
                    }
                }
            }
            // R9: the settings view over the real `lca-config` keys.
            "settings" => {
                let mut text =
                    String::from("settings (key = value [source]; run /grants for permissions):\n");
                for (key, value, source) in self.config.resolved() {
                    text.push_str(&format!("  {key} = {value} [{source}]\n"));
                }
                CommandEffect::ShowWidget(text)
            }
            // The stats story (FR-UI-19): the same numbers the footer
            // accumulates, with per-model cost and cache waste.
            "session" => CommandEffect::ShowWidget(super::display::session_stats(
                &self.store,
                &self.session(),
            )),
            // The generic identity commands dispatch across installed
            // providers first (FR-PROV-11); with zero enabled providers
            // FR-PROV-6's report shows instead.
            "login" | "logout" | "usage" => self
                .registry
                .invoke_generic(name, argument, &self.provider_name)
                .unwrap_or_else(|| {
                    CommandEffect::ShowWidget(crate::no_model_message(&self.provider_name))
                }),
            _ => self
                .registry
                .invoke_command(name, argument)
                .unwrap_or(CommandEffect::None),
        }
    }

    /// `/attach <path>`: stage an image for the next turn (ADR-0029). The
    /// bytes land in the session's attachment store; the stub text rides
    /// with the next user message.
    fn command_attach(&self, argument: &str) -> CommandEffect {
        let session = self.session();
        let path = std::path::Path::new(argument.trim());
        match lca_core::stage_image(&session, path) {
            Ok(staged) => {
                let note = format!(
                    "attached {} - it goes with your next message",
                    &staged.hash[..8]
                );
                self.pending_attachments
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(staged);
                // Show the image placeholder at once (FR-UI-13); fall back to
                // the note when the bytes are gone.
                match std::fs::read(path).ok().and_then(|bytes| {
                    lca_protocol::sniff_image_media_type(&bytes)
                        .map(|media_type| (media_type.to_string(), bytes))
                }) {
                    Some((media_type, bytes)) => CommandEffect::AttachImage {
                        media_type,
                        bytes,
                        note,
                    },
                    None => CommandEffect::ShowWidget(note),
                }
            }
            Err(err) => CommandEffect::ShowWidget(format!("attach failed: {err}")),
        }
    }

    /// `/model`: the picker, or a named switch that moves the live cell, the
    /// footer window, and the compaction backend together.
    fn command_model(&self, argument: &str) -> CommandEffect {
        let models = self.provider.list_models();
        let effect = model_effect_on(
            &models,
            &self.provider_name,
            argument,
            Some(&self.model_cell),
            Some(&self.label_cell),
            self.provider_backend.as_ref(),
        );
        // Keep the footer's window in step with the choice.
        *self
            .context_window_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = self
            .model_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .window as u64;
        effect
    }

    /// The session the interface is showing, cloned out of the cell.
    pub(super) fn session(&self) -> lca_session::Session {
        self.current_session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}
