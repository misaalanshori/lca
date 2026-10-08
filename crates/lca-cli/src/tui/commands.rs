//! The interface's options and built-in command dispatch (S1). The
//! `/model`/`/attach`/`/compact`/`/settings`/`/session`/identity slots were
//! one closure capturing `run`'s locals; they are a method on [`Ui`] now.

use lca_session::Session;
use std::sync::{Arc, Mutex};

use lca_config::ColorMode;
use lca_protocol::CommandEffect;
use lca_provider::Provider as _;
use lca_ui::UiOptions;

use super::BUILTIN_SLOTS;
use super::Ui;
use super::display::model_effect_on;

/// Stage opener `@file` images into the pending attachments (gh
/// #71): they ride with the first submission exactly like `/attach`
/// does. A file that will not stage warns and drops - the interface
/// stays usable, and the warning names the path.
pub(crate) fn stage_initial_attachments(
    session: &Arc<Mutex<Session>>,
    paths: &[std::path::PathBuf],
) -> Vec<lca_core::StagedAttachment> {
    let Ok(session) = session.lock() else {
        return Vec::new();
    };
    let mut staged = Vec::new();
    for path in paths {
        match lca_core::stage_image(&session, path) {
            Ok(attachment) => staged.push(attachment),
            Err(err) => eprintln!("warning: @{}: {err}", path.display()),
        }
    }
    staged
}

impl Ui {
    /// Every model this session offers: every enabled provider's list
    /// (each across its profiles, gh #31), cut to the enabled scope
    /// (`models.enabled` / `--models`, gh #8). A provider contributes
    /// only when it is ready in the `auth check` sense (gh #177) - an
    /// unready provider contributes nothing, silently. Each model tags
    /// its provider in `extras["provider"]` (absent only when the
    /// extension tagged it first), so a cross-provider pick can route.
    /// The picker's listing, the model cycle, and `/model <name>` all
    /// read this one list, so what a user can see is what the keys can
    /// reach; an empty scope is no restriction.
    pub(super) fn offered_models(&self) -> Vec<lca_protocol::ModelInfo> {
        crate::models::filter_enabled(
            self.all_models(),
            crate::lock(&self.config).models_enabled(),
        )
    }

    /// Every model every ready provider offers, unfiltered (gh #204):
    /// the checklist reads this (a disabled model must stay visible so
    /// it can be re-enabled); the cycle reads the filtered view above.
    pub(super) fn all_models(&self) -> Vec<lca_protocol::ModelInfo> {
        let mut catalog = Vec::new();
        for handle in self.registry().enabled() {
            if !handle.worlds().contains(&lca_ext_abi::World::Provider) {
                continue;
            }
            let name = handle.name().to_string();
            if crate::auth::check_status(&self.registry(), &name) != crate::auth::Status::Ready {
                continue;
            }
            // The active provider lists through its own adapter: the
            // login flow persists discoveries into the shared settings
            // cell (ADR-0035), and a fresh adapter would not see them.
            let listed = if name == self.live_name() {
                self.live_provider().list_models()
            } else {
                lca_core::ExtensionProvider::new(handle.clone()).list_models()
            };
            for mut model in listed {
                model
                    .extras
                    .entry("provider".to_string())
                    .or_insert(name.clone());
                catalog.push(model);
            }
        }
        catalog
    }

    /// The interface's options (S1): the static inputs plus the closure the
    /// interface calls for every slash command.
    pub(super) fn options(self: &Arc<Self>) -> UiOptions {
        let ui = self.clone();
        // Snapshot the cells up front: a `crate::lock` temporary inside
        // the literal below would live until the whole struct builds,
        // deadlocking the first later field that locks the same cell
        // (std Mutex is not reentrant).
        let config = crate::lock(&self.config).clone();
        let yolo =
            crate::lock(&self.grants).permission_mode() == lca_permissions::PermissionMode::Yolo;
        let (keybinding_overrides, keybinding_error) =
            crate::load_user_keybindings(&crate::data_dir());
        let invoke_command: lca_ui::CommandInvoker =
            Arc::new(move |name, argument| ui.dispatch_command(name, argument));
        UiOptions {
            model_label: self.label_cell.clone(),
            context_window: self.context_window_cell.clone(),
            thinking: self.thinking_cell.clone(),
            theme: config.ui_theme().unwrap_or("auto").to_string(),
            // R7: themes live with the rest of the agent's data;
            // gh #70 runs add their explicit `--theme` dirs first.
            theme_extra_dirs: crate::invoke::theme_extra_dirs(
                &self.flags,
                &self.cwd,
                &crate::data_dir(),
            ),
            themes: {
                let dirs =
                    crate::invoke::theme_extra_dirs(&self.flags, &self.cwd, &crate::data_dir());
                lca_ui::theme::theme_names_all(&dirs)
            },
            initial_lines: self.initial_head.clone(),
            initial_records: self.initial_records.clone(),
            initial_tail_lines: self.initial_tail.clone(),
            initial_messages: self.initial_messages.clone(),
            models: super::display::model_rows(&self.offered_models(), &self.live_name()),
            // The session's prompt channel (run publishes its sender here)
            // and the consent flow's rows for the picker (gh #31 review).
            prompt_slot: self.prompt_slot.clone(),
            dialog_slot: self.dialog_slot.clone(),
            pending_models: Some({
                let ui = self.clone();
                Arc::new(move || ui.take_pending_models())
            }),
            plain: config.ui_color() == ColorMode::Never,
            yolo,
            thinking_visibility: config
                .thinking_visibility()
                .and_then(lca_ui::transcript::ThinkingVisibility::parse)
                .unwrap_or_default(),
            // gh #32: the configured shape, with the shipped frame as
            // the answer to anything the config layer already refused.
            codeblock_border: config
                .markdown_codeblock_border()
                .parse()
                .unwrap_or_default(),
            invoke_command,
            render_regions: self.render_regions.clone(),
            ui_events: self.ui_events.clone(),
            update_notice: Some(self.update_notice.clone()),
            login: Some(self.login_options()),
            pick_login: Some(self.login_pick()),
            complete_login: Some(self.login_complete()),
            confirm_login_grant: Some(self.login_confirm()),
            confirm_switch: Some(self.switch_confirm()),
            hooks: self.hooks(),
            // Gh #110: bare `-r` opens the picker over the fresh session.
            open_resume_picker: self.resume_picker,
            // FR-UI-21/R2: a fresh session starts on the main screen (the
            // terminal's own selection); `/fullscreen` opts into the
            // alt-screen renderer and persists in `ui.json`, which wins
            // over the config file's `ui.fullscreen` when present (gh
            // #112; the malformed warning rides the transcript head).
            fullscreen: super::hooks::initial_screen_mode(
                &crate::data_dir(),
                config.ui_fullscreen(),
            )
            .0,
            slash_commands: self.slash_commands(),
            workspace: self.cwd.clone(),
            // gh #66: the user key file sits beside the config (the
            // settings ladder's user rung - below flags and env, which
            // have no key surface, and above the built-in defaults).
            keybinding_overrides,
            keybinding_error,
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
        names.insert(7, "/clone".to_string());
        names.insert(8, "/reload".to_string());
        names.insert(7, "/thinking".to_string());
        names.insert(8, "/resume".to_string());
        names.insert(9, "/settings".to_string());
        names.insert(10, "/grants".to_string());
        names.insert(11, "/trust".to_string());
        names.insert(12, "/permissions".to_string());
        names.insert(13, "/scoped-models".to_string());
        names.insert(14, "/changelog".to_string());
        // gh #43: the skill command plus one entry per skill, so
        // `/skill:name` completes and forwards to the host.
        names.push("/skill".to_string());
        for skill in lca_tools::skills::collect(&crate::skills_roots(&self.cwd, &self.flags)) {
            names.push(format!("/skill:{}", skill.name));
        }
        names.extend(
            self.registry()
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
            // gh #43: `/skill` lists, `/skill:name [args]` (or the space
            // form) loads the body and submits it with the args as one
            // user block. Explicit invocation bypasses
            // `disable-model-invocation` - that flag gates the model's
            // paths, never this command.
            name if name == "skill" || name.starts_with("skill:") => {
                self.command_skill(name, argument)
            }
            "compact" => {
                // Summarization is a model round-trip. It used to run on
                // this thread, which is the interface's: the pane went
                // unresponsive for its whole duration (measured: a typed
                // character appeared 3.5 s later, with the notice). It now
                // runs on its own thread and reports back through
                // `poll_compact`, which raises the working state and posts
                // the result - the shape pi gives it with
                // `CompactionStatusIndicator`.
                let mut state = self
                    .compact_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if *state == lca_ui::CompactState::Running {
                    // One compaction at a time; the notice says so.
                    return CommandEffect::ShowWidget(
                        "a compaction is already running".to_string(),
                    );
                }
                *state = lca_ui::CompactState::Running;
                drop(state);

                let state = self.compact_state.clone();
                let store = self.store.clone();
                let session = self.session();
                let live = crate::lock(&self.live).agent_config.clone();
                let extensions = live.extensions.clone();
                let backend = live.completion_backend.clone();
                // Gh #36 phase 3: the manual checkpoint records the
                // prompt the turn would have used.
                let system_prompt = Some(live.system_prompt.clone());
                std::thread::spawn(move || {
                    let notice = match lca_core::compact_now(
                        store,
                        session,
                        extensions,
                        backend,
                        system_prompt,
                    ) {
                        Ok(summary) => format!("compacted: {summary}"),
                        Err(detail) => format!("nothing was compacted: {detail}"),
                    };
                    *state.lock().unwrap_or_else(|p| p.into_inner()) =
                        lca_ui::CompactState::Done(notice);
                });
                CommandEffect::None
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
                // V1 (#20): read the startup mirror, never `tools` - the
                // runner holds that lock for a whole turn, and this arm
                // runs on the input thread (frozen UI, Ctrl+C queued).
                CommandEffect::ShowWidget(settings_text(
                    &crate::lock(&self.config),
                    live.as_deref(),
                    Some(&theme),
                    self.resolved_shell.as_ref(),
                    self.resolved_shell_error.as_deref(),
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
                .registry()
                .invoke_generic(name, argument, &self.live_name())
                .unwrap_or_else(|| {
                    CommandEffect::ShowWidget(crate::no_model_message(&self.live_name()))
                }),
            // R4: a provider's namespaced identity `login` blocks on a
            // browser callback (the owner's freeze). It runs on a
            // background thread; the interface polls the result.
            name if name.ends_with(".login") => {
                let provider = name.trim_end_matches(".login");
                match self.registry().provider(provider).cloned() {
                    Some(handle) => {
                        self.spawn_identity_login(handle);
                        CommandEffect::ShowWidget(
                            "waiting for browser sign-in… (esc cancels)".to_string(),
                        )
                    }
                    None => self
                        .registry()
                        .invoke_command(name, argument)
                        .unwrap_or(CommandEffect::None),
                }
            }
            _ => self
                .registry()
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
    /// `/skill`: list the catalog, or load one skill's body with the
    /// trailing args appended as the user request - one user-visible
    /// block that submits as a turn (pi's `/skill:name` shape).
    fn command_skill(&self, name: &str, argument: &str) -> CommandEffect {
        let roots = crate::skills_roots(&self.cwd, &self.flags);
        let mut rest = name
            .strip_prefix("skill")
            .unwrap_or("")
            .trim_start_matches(':')
            .to_string();
        if !argument.trim().is_empty() {
            if !rest.is_empty() {
                rest.push(' ');
            }
            rest.push_str(argument.trim());
        }
        let mut words = rest.splitn(2, char::is_whitespace);
        let skill_name = words.next().unwrap_or("").trim();
        let args = words.next().unwrap_or("").trim();
        if skill_name.is_empty() {
            let skills = lca_tools::skills::collect(&roots);
            if skills.is_empty() {
                return CommandEffect::ShowWidget("no skills installed".to_string());
            }
            let mut lines = vec!["Skills (load one with /skill:name):".to_string()];
            for skill in &skills {
                let mut line = format!("/skill:{} - ", skill.name);
                if skill.description.is_empty() {
                    line.push_str(&format!("({})", skill.source.label()));
                } else {
                    line.push_str(&format!("{} ({})", skill.description, skill.source.label()));
                }
                lines.push(line);
            }
            return CommandEffect::ShowWidget(lines.join("\n"));
        }
        let found = lca_tools::skills::collect(&roots)
            .into_iter()
            .find(|skill| skill.name == skill_name);
        match found {
            None => CommandEffect::ShowWidget(format!(
                "no skill named `{skill_name}` - /skill lists them"
            )),
            Some(skill) => {
                let mut text = format!(
                    "[skill {} from {}]\n{}",
                    skill.name,
                    skill.source.label(),
                    skill.body
                );
                if !args.is_empty() {
                    text.push_str("\n\n");
                    text.push_str(args);
                }
                CommandEffect::SubmitPrompt(text)
            }
        }
    }

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
    fn command_model(self: &Arc<Self>, argument: &str) -> CommandEffect {
        let models = self.offered_models();
        // EFG-041: `/model sonnet:high` names the level with the model.
        // The suffix splits before resolution (an id that matches as a
        // whole keeps its colons); what it - and a model switch - does to
        // the session's level is decided below, once the target model is
        // known, because the clamp is against *that* model's set.
        let (argument, suffix_level) = {
            let (base, level) = crate::models::split_thinking(argument.trim());
            (base.to_string(), level.map(str::to_string))
        };
        let argument = argument.as_str();
        let resolves = crate::models::resolve_pattern(argument, &models)
            .is_ok_and(|resolved| models.iter().any(|model| model.id == resolved.id));
        // An empty list with an env-configured endpoint means live
        // discovery is about to happen (gh #31 review): consent first,
        // off this thread so the modal can render. The list arrives
        // through `pending_models` once the answer lands, so the picker
        // opens then; until then this says what is happening instead of
        // the misleading "no models".
        if models.is_empty() && argument.trim().is_empty() && self.start_model_consent() {
            let registry = self.registry();
            let host = crate::net_consent::env_configured_host(
                &self.data,
                &self.live_name(),
                Some(registry.as_ref()),
            )
            .unwrap_or_default();
            return CommandEffect::ShowWidget(format!(
                "{host} is not granted yet - approve the prompt to list its models"
            ));
        }
        // Gh #177: a pick from another ready provider swaps the whole
        // generation (provider, model, backend, record), not just the
        // cell - the pick IS the confirmation, the way pi switches on
        // selection.
        if resolves
            && let Ok(resolved) = crate::models::resolve_pattern(argument, &models)
            && let Some(picked) = models.iter().find(|model| model.id == resolved.id)
            && picked
                .extras
                .get("provider")
                .is_some_and(|owner| *owner != self.live_name())
        {
            let target = picked.extras.get("provider").cloned().unwrap_or_default();
            return self.command_model_switch(
                &target,
                &picked.id,
                picked.context_window,
                picked.extras.get("profile").cloned(),
                suffix_level.as_deref(),
            );
        }
        // E5: the label keeps the preset identity across a model switch.
        let identity = self
            .identity_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        // The model the session ran on before this command: gh #8's
        // `model-change` record names it, and only a *change* appends one.
        let previous = self
            .model_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .id
            .clone();
        let effect = model_effect_on(
            &models,
            &identity,
            &self.live_name(),
            argument,
            Some(&self.model_cell),
            Some(&self.label_cell),
            self.provider_backend.as_ref(),
        );
        // Keep the footer's window in step with the choice.
        let chosen = self
            .model_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        *self
            .context_window_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = chosen.window as u64;
        // gh #8 phase 4: the session's thinking follows the model it runs
        // on. An explicit `:level` is the user's choice, clamped into what
        // the model accepts; a model *switch* applies the new model's
        // configured default (pi's per-model precedence), else keeps the
        // session's level, clamped. A pattern that resolved to nothing
        // changes neither model nor level.
        if resolves {
            let current = self
                .thinking_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let effective = match suffix_level.as_deref() {
                Some(level) => crate::lock(&self.config).clamp_thinking(Some(level), &chosen.id),
                None if chosen.id != previous => {
                    crate::lock(&self.config).switch_thinking(current.as_deref(), &chosen.id)
                }
                None => current,
            };
            *self.thinking_cell.lock().unwrap_or_else(|p| p.into_inner()) = effective;
        }
        // ADR-0024: a switch moves the model everywhere it is read, and
        // `meta.json` is one of those readers' source (gh #20) - written
        // the moment the model is chosen, so a session that switches and
        // then closes says what it ran on. gh #8: the same moment appends
        // a `model-change` record, but only when the model actually
        // changed - reopening the picker or re-picking the active model is
        // no change.
        if !argument.trim().is_empty() && !chosen.id.is_empty() {
            let session = self.session();
            if chosen.id != previous {
                let profile = models
                    .iter()
                    .find(|model| model.id == chosen.id)
                    .and_then(|model| model.extras.get("profile").cloned());
                let record = crate::models::model_change_record(
                    Some(&previous),
                    &chosen.id,
                    &self.live_name(),
                    profile.as_deref(),
                );
                if let Err(err) = self.store.append(&session, record) {
                    return CommandEffect::ShowWidget(format!(
                        "cannot record the model change: {err}"
                    ));
                }
            }
            if let Err(err) = self
                .store
                .record_model_used(&session, &self.live_name(), &chosen.id)
            {
                return CommandEffect::ShowWidget(format!(
                    "cannot update the session metadata: {err}"
                ));
            }
        }
        effect
    }

    /// A `/model` pick from another ready provider (gh #177): swap the
    /// live generation - name, provider, agent config - then follow with
    /// the compaction backend and every cell, the thinking level, the
    /// `model-change` record carrying both sides, and `meta.json`. The
    /// pick is explicit, so no further confirmation asks.
    fn command_model_switch(
        self: &Arc<Self>,
        target: &str,
        model_id: &str,
        window: u32,
        profile: Option<String>,
        suffix_level: Option<&str>,
    ) -> CommandEffect {
        match self.apply_provider_switch(target, model_id, window, profile, suffix_level) {
            Ok(notice) => CommandEffect::ShowWidget(notice),
            Err(err) => CommandEffect::ShowWidget(err),
        }
    }

    /// Move this session to another provider (gh #177): the one routine
    /// both `/model` and the `/login` switch confirmation share. Returns
    /// the notice naming what now answers, or the refusal.
    fn apply_provider_switch(
        &self,
        target: &str,
        model_id: &str,
        window: u32,
        profile: Option<String>,
        suffix_level: Option<&str>,
    ) -> Result<String, String> {
        let Some(handle) = self.registry().provider(target).cloned() else {
            return Err(format!("no provider named `{target}`"));
        };
        let previous = self
            .model_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .id
            .clone();
        let previous_provider = self.live_name();
        let provider = Arc::new(lca_core::ExtensionProvider::new_with_settings(
            handle,
            self.settings_cell.clone(),
        ));
        let listed = provider.list_models();
        if !listed.iter().any(|model| model.id == model_id) {
            return Err(format!("no model named `{model_id}` for {target}"));
        }
        // The new generation's identity reads like startup's (E5): the
        // stored login preset when one is known, else the extension.
        let identity =
            crate::stored_provider_preset(&self.data, target).unwrap_or_else(|| target.to_string());
        super::adopt_login_identity(&self.identity_cell, &identity);
        *self.model_cell.lock().unwrap_or_else(|p| p.into_inner()) = super::ModelChoice {
            id: model_id.to_string(),
            window,
        };
        *self
            .context_window_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = u64::from(window);
        *self.label_cell.lock().unwrap_or_else(|p| p.into_inner()) =
            format!("{identity}/{model_id}");
        if let Some(backend) = self.provider_backend.as_ref() {
            backend.set_provider(provider.clone());
            backend.set_model(model_id.to_string());
        }
        {
            let mut live = crate::lock(&self.live);
            live.name = target.to_string();
            live.provider = provider.clone();
            live.agent_config.provider = target.to_string();
            live.agent_config.model = model_id.to_string();
            live.agent_config.model_context_window = window;
            live.agent_config.image_policy = crate::models::image_policy_for(&listed, model_id);
        }
        // The session's thinking follows the model it runs on (gh #8
        // phase 4, same rule as a same-provider switch).
        let current = self
            .thinking_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let effective = match suffix_level {
            Some(level) => crate::lock(&self.config).clamp_thinking(Some(level), model_id),
            None if model_id != previous => {
                crate::lock(&self.config).switch_thinking(current.as_deref(), model_id)
            }
            None => current,
        };
        *self.thinking_cell.lock().unwrap_or_else(|p| p.into_inner()) = effective;
        // The log's witness carries both sides (gh #8, gh #177); only an
        // actual change appends one. `meta.json` follows, the way a
        // same-provider switch writes it (ADR-0024).
        let session = self.session();
        if model_id != previous || target != previous_provider {
            let record = crate::models::model_change_record(
                Some(&previous),
                model_id,
                target,
                profile.as_deref(),
            );
            if let Err(err) = self.store.append(&session, record) {
                return Err(format!("cannot record the model change: {err}"));
            }
        }
        if let Err(err) = self.store.record_model_used(&session, target, model_id) {
            return Err(format!("cannot update the session metadata: {err}"));
        }
        Ok(format!("model for this session: {target}/{model_id}"))
    }

    /// Answer the provider-switch confirmation after a `/login` (gh
    /// #177): keep the session's model when the new provider offers
    /// it, else fall back to that provider's default the way startup
    /// resolves it. A decline never reaches here (the interface
    /// answers it), so this always moves.
    pub(super) fn switch_confirm(self: &Arc<Self>) -> lca_ui::SwitchConfirm {
        let ui = self.clone();
        Arc::new(move |provider: &str| -> String {
            let Some(handle) = ui.registry().provider(provider).cloned() else {
                return format!("no provider named `{provider}`");
            };
            let adapter =
                lca_core::ExtensionProvider::new_with_settings(handle, ui.settings_cell.clone());
            let listed = adapter.list_models();
            let current = ui
                .model_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .id
                .clone();
            let default_id = super::resolve_model_id(&crate::lock(&ui.config), true, &adapter);
            let pick = listed
                .iter()
                .find(|model| model.id == current)
                .or_else(|| listed.iter().find(|model| model.id == default_id));
            let Some(pick) = pick else {
                return format!("`{provider}` offers no models");
            };
            let (id, window, profile) = (
                pick.id.clone(),
                pick.context_window,
                pick.extras.get("profile").cloned(),
            );
            match ui.apply_provider_switch(provider, &id, window, profile, None) {
                Ok(notice) => notice,
                Err(err) => err,
            }
        })
    }

    /// One step of the model cycle (gh #8, pi's `cycleForward` /
    /// `cycleBackward`): the next or previous model of the enabled scope,
    /// wrapping, applied through `command_model` - so a cycle is a
    /// `/model` switch in every respect (same cells, same footer, same
    /// `model-change` record, same `meta.model` write). Returns the
    /// notice to show; pi's two singleton messages included.
    pub(super) fn cycle_model(self: &Arc<Self>, forward: bool) -> String {
        let models = self.offered_models();
        let current = self
            .model_cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .id
            .clone();
        let index = models.iter().position(|model| model.id == current);
        let Some(next) = crate::models::cycle_index(index, models.len(), forward) else {
            return if crate::lock(&self.config).models_enabled().is_empty() {
                "only one model available".to_string()
            } else {
                "only one model in scope".to_string()
            };
        };
        let target = models[next].id.clone();
        match self.command_model(&target) {
            CommandEffect::ShowWidget(text) => text,
            _ => format!("model for this session: {}", self.live_name()),
        }
    }

    /// Take the rows the endpoint consent discovered for the picker (one
    /// shot: the cell clears as the rows leave it).
    fn take_pending_models(&self) -> Vec<lca_ui::ModelRow> {
        self.pending_models
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .unwrap_or_default()
    }

    /// The picker's endpoint consent (gh #31 review): every live request
    /// to a profile endpoint runs through `endpoint_consent` first, and
    /// `/model`'s `GET /models` discovery is one. Returns whether the ask
    /// is now in flight.
    ///
    /// The ask runs on its own thread because the interface must keep
    /// painting for the modal to appear - the same reason a turn's consent
    /// is not answered on the loop's thread. A granted host never gets
    /// here: `endpoint_consent` itself costs one grant-store read.
    fn start_model_consent(self: &Arc<Self>) -> bool {
        let registry = self.registry();
        let Some(host) = crate::net_consent::env_configured_host(
            &self.data,
            &self.live_name(),
            Some(registry.as_ref()),
        ) else {
            return false;
        };
        if crate::ungranted_host(&self.grants, &self.cwd, Some(host.clone())).is_none() {
            return false; // already granted: discovery is free to run
        }
        let in_flight = self.consent_in_flight.clone();
        if in_flight.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return true; // already asking
        }
        let ui = self.clone();
        std::thread::spawn(move || {
            let session = ui.session();
            let mut prompt = super::SessionPrompt {
                slot: ui.prompt_slot.clone(),
            };
            let outcome = crate::net_consent::endpoint_consent(
                &host,
                &ui.grants,
                &ui.cwd,
                &mut prompt,
                &ui.store,
                &session,
            );
            // At most one message: the flow explains every answer
            // instead of leaving the picker closed behind a stale notice
            // (gh #31 review). An answer that grants nothing says so; the
            // one that granted this session says what it granted.
            let ungranted = format!(
                "the endpoint {host} is not granted for this project - approve it or run /login"
            );
            let message = match outcome {
                crate::net_consent::EndpointConsent::Denied => Some(ungranted),
                // `trust folder` answers "allowed" but attaches no host
                // - it is not a host's grant to give - so the discovery
                // request that follows would be refused anyway.
                _ if crate::ungranted_host(&ui.grants, &ui.cwd, Some(host.clone())).is_some() => {
                    Some(ungranted)
                }
                _ => {
                    // The grant landed: discovery (the extension's live
                    // `GET /models`) now runs inside `list_models`, and
                    // the rows go to the picker through its one-shot cell.
                    let identity = ui
                        .identity_cell
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone();
                    let rows =
                        super::display::model_rows(&ui.live_provider().list_models(), &identity);
                    if rows.is_empty() {
                        Some(format!("no models were offered by {host}"))
                    } else {
                        // `once` joined the session set (the same one
                        // `--allow-host` uses): say what that granted.
                        let session_only =
                            crate::lock(&ui.grants).session_net_pattern(&ui.cwd, &host);
                        *ui.pending_models.lock().unwrap_or_else(|p| p.into_inner()) = Some(rows);
                        session_only.then(|| {
                            format!("{host} allowed for this session - a new run asks again")
                        })
                    }
                }
            };
            if let Some(message) = message {
                *ui.login_pending.lock().unwrap_or_else(|p| p.into_inner()) =
                    Some(lca_ui::LoginNext::Message(message));
            }
            in_flight.store(false, std::sync::atomic::Ordering::SeqCst);
        });
        true
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
/// The read-only dump (FR-CFG-2, gh #30): every key with its winning
/// source, the session's live overrides labelled, `shell.resolved`, and
/// the screen renderer. `lca config` prints it; the `/settings`
/// selector's fallback prints it for a host that ships no rows.
pub(crate) fn settings_text(
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
    // S1: the active screen renderer mode - persisted ui.json wins
    // when present (gh #112), else the config file's ui.fullscreen.
    // The source names the layer that won, not a constant.
    let (is_fullscreen, _) =
        super::hooks::initial_screen_mode(&crate::data_dir(), config.ui_fullscreen());
    let source = if crate::data_dir().join("ui.json").exists() {
        "ui.json"
    } else if config.ui_fullscreen().is_some() {
        "config"
    } else {
        "default"
    };
    let mode_desc = if is_fullscreen {
        format!("app-owned screen (fullscreen) [{source}]")
    } else {
        format!("terminal scrollback [{source}]")
    };
    text.push_str(&format!("  ui.screen = {mode_desc}\n"));
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
