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
            initial_lines: self.initial_head.clone(),
            initial_records: self.initial_records.clone(),
            initial_tail_lines: self.initial_tail.clone(),
            models: self
                .provider
                .list_models()
                .iter()
                .map(|model| model.id.clone())
                .collect(),
            plain: self.config.ui_color() == ColorMode::Never,
            yolo: crate::lock(&self.grants).permission_mode()
                == lca_permissions::PermissionMode::Yolo,
            invoke_command,
            render_regions: self.render_regions.clone(),
            ui_events: self.ui_events.clone(),
            update_notice: Some(self.update_notice.clone()),
            login: Some(self.login_options()),
            pick_login: Some(self.login_pick()),
            complete_login: Some(self.login_complete()),
            confirm_login_grant: Some(self.login_confirm()),
            hooks: self.hooks(),
            // FR-UI-21/R2: a fresh session starts on the main screen (the
            // terminal's own selection); `/fullscreen` opts into the
            // alt-screen renderer and persists in `ui.json`.
            fullscreen: super::hooks::initial_screen_mode(&crate::config_dir()),
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
        names.insert(11, "/trust".to_string());
        names.insert(12, "/permissions".to_string());
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
    fn dispatch_command(self: &Arc<Self>, name: &str, argument: &str) -> CommandEffect {
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
            // ADR-0039: allow/deny rules with global defaults.
            "permissions" => self.command_permissions(argument),
            // R9 + E2: the settings view over the real `lca-config` keys,
            // with the session's live thinking and theme overriding the file's.
            "settings" => {
                let live = self
                    .thinking_cell
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                let theme = self
                    .theme_cell
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                let (shell, shell_error) = {
                    let tools = crate::lock(&self.tools);
                    (
                        tools.resolved_shell().cloned(),
                        tools.resolved_shell_error().map(str::to_string),
                    )
                };
                CommandEffect::ShowWidget(settings_text(
                    &self.config,
                    live.as_deref(),
                    Some(&theme),
                    shell.as_ref(),
                    shell_error.as_deref(),
                ))
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
            // R4: a provider's namespaced identity `login` blocks on a
            // browser callback (the owner's freeze). It runs on a
            // background thread; the interface polls the result.
            name if name.ends_with(".login") => {
                let provider = name.trim_end_matches(".login");
                match self.registry.provider(provider).cloned() {
                    Some(handle) => {
                        self.spawn_identity_login(handle);
                        CommandEffect::ShowWidget(
                            "waiting for browser sign-in… (esc cancels)".to_string(),
                        )
                    }
                    None => self
                        .registry
                        .invoke_command(name, argument)
                        .unwrap_or(CommandEffect::None),
                }
            }
            _ => self
                .registry
                .invoke_command(name, argument)
                .unwrap_or(CommandEffect::None),
        }
    }

    /// `/permissions` (ADR-0039): manage allow/deny rules.
    ///
    /// ```text
    /// /permissions                          list rules
    /// /permissions [session|project|global] allow <glob>
    /// /permissions [session|project|global] deny  <glob>
    /// /permissions clear                    drop session rules
    /// ```
    ///
    /// The scope defaults to project; global rules are the user's defaults
    /// across every project, session rules vanish on exit.
    fn command_permissions(&self, argument: &str) -> CommandEffect {
        use lca_permissions::{RuleDecision, RuleScope};
        let mut parts = argument.split_whitespace();
        let first = parts.next().unwrap_or("list");
        if first == "list" {
            let store = self.grants.lock().unwrap_or_else(|p| p.into_inner());
            let rules = store.rules(&self.cwd);
            if rules.is_empty() {
                return CommandEffect::ShowWidget(
                    "no permission rules; e.g. `/permissions deny git push*`".to_string(),
                );
            }
            let mut text = String::from("permission rules (deny beats allow):\n");
            for rule in rules {
                let scope = match rule.scope {
                    RuleScope::Session => "session",
                    RuleScope::Project => "project",
                    RuleScope::Global => "global",
                };
                let decision = match rule.decision {
                    RuleDecision::Allow => "allow",
                    RuleDecision::Deny => "deny",
                };
                text.push_str(&format!("  [{scope}] {decision} {}\n", rule.pattern));
            }
            return CommandEffect::ShowWidget(text);
        }
        if first == "clear" {
            self.grants
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clear_session_rules();
            return CommandEffect::ShowWidget("cleared session rules".to_string());
        }
        // `[scope] allow|deny <glob>`; the scope is optional.
        let (scope, decision_word) = match first {
            "session" | "project" | "global" => (Some(first), parts.next().unwrap_or("")),
            "allow" | "deny" => (None, first),
            _ => (None, ""),
        };
        let pattern: String = parts.collect::<Vec<_>>().join(" ");
        if decision_word.is_empty() || pattern.is_empty() {
            return CommandEffect::ShowWidget(
                "usage: /permissions [session|project|global] allow|deny <glob>".to_string(),
            );
        }
        let rule_scope = match scope {
            Some("session") => RuleScope::Session,
            Some("global") => RuleScope::Global,
            _ => RuleScope::Project,
        };
        let decision = match decision_word {
            "allow" => RuleDecision::Allow,
            "deny" => RuleDecision::Deny,
            other => {
                return CommandEffect::ShowWidget(format!(
                    "unknown decision `{other}`; use allow or deny"
                ));
            }
        };
        let mut store = self.grants.lock().unwrap_or_else(|p| p.into_inner());
        match store.add_rule(&self.cwd, rule_scope, decision, pattern.clone()) {
            Ok(()) => CommandEffect::ShowWidget(format!("{decision_word} rule `{pattern}`")),
            Err(err) => CommandEffect::ShowWidget(format!("could not add rule: {err}")),
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
        // E5: the label keeps the preset identity across a model switch.
        let identity = self
            .identity_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let effect = model_effect_on(
            &models,
            &identity,
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

/// The `/settings` text (E2): every resolved key with its source, with the
/// session's live thinking value overriding the file's. The live value is
/// labeled `session`; with neither a session pick nor a file value the row
/// reads `unset (provider default) [default]`.
fn settings_text(
    config: &lca_config::Config,
    live_thinking: Option<&str>,
    live_theme: Option<&str>,
    shell: Option<&lca_tools::Shell>,
    shell_error: Option<&str>,
) -> String {
    let configured = config.thinking();
    let configured_theme = config.ui_theme().unwrap_or("auto");
    let mut text = String::from("settings (key = value [source]; run /grants for permissions):\n");
    for (key, value, source) in config.resolved() {
        if key == "thinking" {
            if live_thinking != configured {
                match live_thinking {
                    Some(level) => {
                        text.push_str(&format!("  thinking = {level} [session]\n"));
                        continue;
                    }
                    None => {
                        text.push_str("  thinking = unset (provider default) [session]\n");
                        continue;
                    }
                }
            }
            if live_thinking.is_none() {
                text.push_str("  thinking = unset (provider default) [default]\n");
                continue;
            }
        }
        if key == "ui.theme"
            && let Some(live) = live_theme
            && live != configured_theme
        {
            text.push_str(&format!("  ui.theme = {live} [session]\n"));
            continue;
        }
        text.push_str(&format!("  {key} = {value} [{source}]\n"));
    }
    // ADR-0041: the key rows above are what was asked for; this row is what
    // the ladder actually resolved, which is the thing the model's tool
    // description names.
    match (shell, shell_error) {
        // A broken backend must not look resolved: every call fails.
        (_, Some(error)) => text.push_str(&format!("  shell.resolved = <unresolved: {error}>\n")),
        (Some(shell), None) => text.push_str(&format!("  shell.resolved = {shell}\n")),
        (None, None) => text.push_str("  shell.resolved = <host-delegated>\n"),
    }
    text
}

#[cfg(test)]
mod tests {
    use super::settings_text;
    use lca_config::{Config, LoadInput};

    fn config_with(name: &str, text: &str) -> Config {
        let dir = lca_testkit::scratch_path(name);
        let path = dir.join("config.toml");
        std::fs::write(&path, text).expect("seed config");
        let input = LoadInput {
            user_file: Some(path),
            ..Default::default()
        };
        Config::load(&input).expect("load")
    }

    // Verifies: ADR-0041 - a broken backend is not shown as resolved: the
    // row names the error, because every shell call fails until it is fixed.
    #[test]
    fn settings_show_an_unresolved_shell_as_broken() {
        let config = config_with("lca-settings-shell", "");
        let text = settings_text(
            &config,
            None,
            None,
            None,
            Some("shell.path does not name a file: /nope"),
        );
        assert!(
            text.contains("shell.resolved = <unresolved: shell.path does not name a file: /nope>"),
            "{text}"
        );
    }

    // Verifies: E2 - `/settings` labels the session's live thinking pick.
    #[test]
    fn settings_label_the_session_thinking_pick() {
        let config = config_with("lca-settings-live", "");
        let text = settings_text(&config, Some("high"), None, None, None);
        assert!(text.contains("thinking = high [session]"), "{text}");
    }

    // Verifies: E2 - with no pick and no file value, the row is the honest
    // `unset (provider default)`.
    #[test]
    fn settings_show_unset_thinking() {
        let config = config_with("lca-settings-unset", "");
        let text = settings_text(&config, None, None, None, None);
        assert!(
            text.contains("thinking = unset (provider default) [default]"),
            "{text}"
        );
    }

    // Verifies: E2 - once the file carries the value (a restart), the row
    // reads it from the file and agrees with the live cell.
    #[test]
    fn settings_agree_after_a_restart() {
        let config = config_with("lca-settings-restart", "thinking = \"high\"\n");
        let text = settings_text(&config, Some("high"), None, None, None);
        assert!(text.contains("thinking = high [user file]"), "{text}");
    }

    // Verifies: E2 - `/settings` labels a committed `/theme` pick the same
    // way it labels thinking, so the two surfaces agree there too.
    #[test]
    fn settings_label_the_session_theme_pick() {
        let config = config_with("lca-settings-theme", "");
        let text = settings_text(&config, None, Some("light"), None, None);
        assert!(text.contains("ui.theme = light [session]"), "{text}");
    }
}
