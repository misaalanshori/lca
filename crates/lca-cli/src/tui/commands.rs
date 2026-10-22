//! The interface's options and built-in command dispatch (S1). The
//! `/model`/`/attach`/`/compact`/`/settings`/`/session`/identity slots were
//! one closure capturing `run`'s locals; they are a method on [`Ui`] now.

use std::sync::Arc;

use lca_config::ColorMode;
use lca_protocol::CommandEffect;
use lca_provider::Provider as _;
use lca_ui::UiOptions;

use super::BUILTIN_SLOTS;
use super::Ui;
use super::model_select::pick_model;

/// Every slash name the host's [`Ui::dispatch_command`] answers itself,
/// without the extension registry. One list, three readers: the router's
/// allowlist (`slash_commands`), `/help`'s description table (which must
/// carry a line for each), and the guard that keeps the two honest
/// (gh #443).
///
/// **Gh #443: this is the fix for the class, not one instance.** The
/// router (`Chat::dispatch_command`) admits a name only when it is in
/// `slash_commands`, so a handler that is not registered is dead code
/// that `/help` still advertises - `/bug`, `/export`, and `/import` were
/// exactly that, answering "unknown command" while their implementations
/// sat unreachable. Listing them here registers them and keeps the guard
/// (in this module's tests) able to prove no future handler is orphaned.
///
/// The list is deliberately *not* the same as `slash_commands()`: that
/// one also carries extension-registered commands and `/skill:<name>`
/// instances, which no static table can enumerate.
pub(crate) const HANDLED_NAMES: &[&str] = &[
    "attach",
    "bug",
    "compact",
    "export",
    "import",
    "login",
    "logout",
    "model",
    "paste",
    "permissions",
    "session",
    "settings",
    "skill",
    "usage",
];

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
        let models = crate::models::filter_enabled(
            self.all_models(),
            crate::lock(&self.config).models_enabled(),
        );
        // The pick path's memo (gh #240): every enumeration refreshes
        // it, so a picker Enter right after happily skips its own.
        // Lock order is registry-then-cache everywhere (the pick path
        // clones the Arc first too): std Mutex is not reentrant.
        let registry = self.registry();
        self.offered_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .fill(&registry, models.clone());
        models
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
            // Gh #400: hand the pre-load terminal and its buffer to the loop
            // exactly once; a second `options()` gets `None` and would start
            // its own terminal, which only a bug would reach.
            early_input: crate::lock(&self.early_input).take(),
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
            // gh #76: queued follow-ups flush per `queue.follow_up_mode`.
            follow_up_mode: lca_protocol::QueueMode::parse(config.queue_follow_up_mode())
                .unwrap_or_default(),
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
    /// built-in slots, then every command the host handles itself, then
    /// every extension command (sanitized).
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
        names.insert(14, "/mcp".to_string());
        names.insert(15, "/changelog".to_string());
        // gh #75: pi's `/name` completes like the `/rename` it names.
        names.insert(16, "/name".to_string());
        // gh #43: the skill command plus one entry per skill, so
        // `/skill:name` completes and forwards to the host.
        names.push("/skill".to_string());
        // #227: the Extension Hub completes like any host command.
        names.push("/extensions".to_string());
        // Gh #443: whatever the host itself answers, the router must
        // admit. Derived from `HANDLED_NAMES` rather than hand-pasted, so
        // a handler and its registration cannot drift apart again - the
        // guard in this module's tests fails on any name the dispatcher
        // answers that this list does not contain.
        for name in HANDLED_NAMES {
            names.push(format!("/{name}"));
        }
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
            "paste" => self.command_paste(),
            "export" => self.command_export(argument),
            "import" => self.command_import(argument),
            "bug" => self.command_bug(),
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
                if matches!(*state, lca_ui::CompactState::Running(_)) {
                    // One compaction at a time; the notice says so.
                    return CommandEffect::ShowWidget(
                        "a compaction is already running".to_string(),
                    );
                }
                // The user asked for it, so the separator names it (gh #388).
                *state = lca_ui::CompactState::Running(lca_ui::CompactionReason::Manual);
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
            // FR-PROV-6's report shows instead. The op runs off the
            // interface thread (#102): progress now, the result through
            // `poll_login`.
            "login" | "logout" | "usage" => self.command_identity(name, argument),
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

    /// The generic `/login`, `/logout`, `/usage` (FR-PROV-11): the
    /// identity op is a provider round-trip, so it runs on a background
    /// thread (#102) and the interface answers progress at once. The
    /// result arrives as a notice through `poll_login`. Zero providers
    /// stays synchronous: it is a local, instant answer (FR-PROV-6).
    fn command_identity(self: &Arc<Self>, name: &str, argument: &str) -> CommandEffect {
        if self.registry().provider_names().is_empty() {
            return CommandEffect::ShowWidget(crate::no_model_message(&self.live_name()));
        }
        // Gh #240: login/logout flip provider readiness, which the
        // model memo cannot see — clear eagerly (a stale miss only
        // re-enumerates once; `usage` changes nothing).
        if matches!(name, "login" | "logout") {
            self.offered_cache
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clear();
        }
        let step = super::pending_step::PendingStep::wrap(
            self.login_pending.clone(),
            self.login_generation.clone(),
        );
        let ticket = step.issue();
        let registry = self.registry();
        let command = name.to_string();
        let argument = argument.to_string();
        let live = self.live_name();
        step.run(ticket, move || {
            match registry.invoke_generic(&command, &argument, &live) {
                Some(CommandEffect::ShowWidget(text)) => lca_ui::LoginNext::Message(text),
                Some(_) => lca_ui::LoginNext::Message(format!("{command} done")),
                None => lca_ui::LoginNext::Message(crate::no_model_message(&live)),
            }
        });
        CommandEffect::ShowWidget(
            match name {
                "login" => "signing in…",
                "logout" => "signing out…",
                _ => "fetching usage…",
            }
            .to_string(),
        )
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
        // Gh #261: what the user typed is *compiled* into a pattern the matcher
        // can actually compare. `/permissions deny read muffin-recipe.txt` used
        // to store the verb as part of the glob (`read muffin-recipe.txt`), which
        // no action value ever equals, so the rule matched nothing and the read
        // ran while the confirmation said it had worked.
        let compiled = match lca_permissions::rule_entry::compile(&pattern, &self.cwd) {
            Ok(compiled) => compiled,
            // A rule that cannot match anything is refused *here*, with the
            // accepted forms named - storing a dead rule is how this bug happened.
            Err(rejection) => return CommandEffect::ShowWidget(rejection.to_string()),
        };
        let mut store = self.grants.lock().unwrap_or_else(|p| p.into_inner());
        match store.add_rule(&self.cwd, rule_scope, decision, compiled.pattern.clone()) {
            Ok(()) => CommandEffect::ShowWidget(confirm_rule(decision_word, &compiled)),
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
            let pick = pick_model(&listed, &current, &default_id);
            let Some(pick) = pick else {
                return format!("`{provider}` offers no models");
            };
            let (id, window, profile) = (
                pick.id.clone(),
                pick.context_window,
                pick.extras.get("profile").cloned(),
            );
            match ui.apply_provider_switch(provider, &id, window, profile, None, None) {
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
    /// The picker's endpoint consent (gh #31 review): every live request
    /// to a profile endpoint runs through `endpoint_consent` first, and
    /// `/model`'s `GET /models` discovery is one. Returns whether the ask
    /// is now in flight.
    ///
    /// The ask runs on its own thread because the interface must keep
    /// painting for the modal to appear - the same reason a turn's consent
    /// is not answered on the loop's thread. A granted host never gets
    /// here: `endpoint_consent` itself costs one grant-store read.
    pub(super) fn start_model_consent(self: &Arc<Self>) -> bool {
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

    /// `/export [html]`: write the current session out (gh #57).
    /// Plain `/export` writes the JSON; `/export html` adds the
    /// self-contained transcript. The notice names the files.
    fn command_export(&self, argument: &str) -> CommandEffect {
        let session = self.session();
        let store = self.store.clone();
        let audit = false;
        let json = match store.export(&session, lca_session::ExportOptions { audit }) {
            Ok(path) => path,
            Err(err) => return CommandEffect::ShowWidget(format!("export failed: {err}")),
        };
        if argument.trim() == "html" {
            match store.export_html(&session, lca_session::ExportOptions { audit }) {
                Ok(html) => CommandEffect::ShowWidget(format!(
                    "exported {}\n{}",
                    json.display(),
                    html.display()
                )),
                Err(err) => CommandEffect::ShowWidget(format!("export failed: {err}")),
            }
        } else {
            CommandEffect::ShowWidget(format!("exported {}", json.display()))
        }
    }

    /// `/import <file>`: JSONL lines become a new session (gh #57).
    /// The notice names the id; resuming it is one `/resume` away.
    fn command_import(&self, argument: &str) -> CommandEffect {
        let path = std::path::Path::new(argument.trim());
        if argument.trim().is_empty() {
            return CommandEffect::ShowWidget("usage: /import <file.jsonl>".to_string());
        }
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => return CommandEffect::ShowWidget(format!("import failed: {err}")),
        };
        let lines: Vec<&str> = text.lines().collect();
        let title = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "imported".to_string());
        match self.store.import_jsonl(&self.cwd, &title, &lines) {
            Ok(session) => CommandEffect::ShowWidget(format!(
                "imported as {} (resume with /resume {})",
                session.id(),
                session.id()
            )),
            Err(err) => CommandEffect::ShowWidget(format!("import failed: {err}")),
        }
    }

    /// `/bug`: the local diagnostic bundle (gh #57), scrubbed before it is
    /// written (gh #411 gap 3).
    ///
    /// The **interactive** flow - stage, inspect, confirm - lives in `lca-ui`'s
    /// own dispatcher, where the modal state and the transcript are, and reaches
    /// the bundle through the `stage_bug` hook. This arm is the fallback a
    /// non-interactive host uses: the same scrub, no gate.
    fn command_bug(self: &Arc<Self>) -> CommandEffect {
        let session = self.session();
        match crate::bug::bundle(&crate::data_dir(), &self.cwd, Some(session.id()), &self.cwd) {
            Ok(path) => CommandEffect::ShowWidget(format!(
                "wrote {} (scrubbed; contains the transcript - share it explicitly)",
                path.display()
            )),
            Err(err) => CommandEffect::ShowWidget(format!("bug failed: {err}")),
        }
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
        // Gh #218: substituted values may be secrets - the source
        // column stays honest, the value never prints.
        if config.substituted(key) {
            text.push_str(&format!("  {key} = ******** [{source}]\n"));
        } else {
            text.push_str(&format!("  {key} = {value} [{source}]\n"));
        }
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

#[cfg(test)]
mod subst_tests {
    // Verifies: gh #218 (`lca config` masks substituted secrets: the
    // source prints, the value never does).
    #[test]
    fn settings_mask_substituted_secrets() {
        let dir = lca_testkit::scratch_path("lca-settings-mask");
        std::fs::write(dir.join("token.txt"), "s3cr3t\n").expect("seed");
        std::fs::write(dir.join("config.toml"), "model = \"{file:token.txt}\"\n").expect("seed");
        let config = lca_config::Config::load(&lca_config::LoadInput {
            user_file: Some(dir.join("config.toml")),
            ..Default::default()
        })
        .expect("load");
        let text = super::settings_text(&config, None, None, None, None);
        assert!(text.contains("model = ******** [user file]"), "{text}");
        assert!(!text.contains("s3cr3t"), "the secret never prints: {text}");
    }
}

/// Guards for gh #443: the router's allowlist, the host's dispatch arms,
/// and `/help`'s description table must all agree.
///
/// The bug these exist for: `slash_commands` was doing double duty as the
/// autocomplete list *and* the router's allowlist, so a handled-but-
/// unlisted name was dead code that `/help` still advertised, and a
/// listed-but-unhandled name fell through to the extension dispatcher and
/// silently did nothing. Both directions are checked here.
#[cfg(test)]
mod command_registration_guards {
    use super::HANDLED_NAMES;

    /// The host's dispatch arms, read from the source so the guard cannot
    /// be satisfied by editing the very list it checks. A helper rather
    /// than a line-by-line grep because `cargo fmt` reflows the match.
    fn dispatch_arms() -> Vec<String> {
        let source = include_str!("commands.rs");
        let start = source
            .find("fn dispatch_command(self: &Arc<Self>")
            .expect("the host dispatcher exists");
        let body = &source[start..];
        let end = body.find("\n    }\n").expect("the dispatcher has a body");
        let mut arms = Vec::new();
        for line in body[..end].lines() {
            let trimmed = line.trim();
            // Guard arms (`name if name == "skill" || name.starts_with("skill:")`)
            // are matched by a condition rather than a literal at the head,
            // so read the quoted names out of the condition too. Without
            // this the scanner reports `/skill` as having no arm at all.
            if trimmed.starts_with("name if ") && trimmed.contains("=>") {
                for (index, part) in trimmed.split('"').enumerate() {
                    if index % 2 == 1 && !part.contains(' ') && !part.ends_with(':') {
                        arms.push(part.to_string());
                    }
                }
                continue;
            }
            if !trimmed.starts_with('"') || !trimmed.contains("=>") {
                continue;
            }
            // `"attach" => ...` and the group `"login" | "logout" | "usage" =>`
            for (index, part) in trimmed.split('"').enumerate() {
                if index % 2 == 1 && !part.contains(' ') {
                    arms.push(part.to_string());
                }
            }
        }
        arms.sort();
        arms.dedup();
        arms
    }

    // Verifies: gh #443 - every name the host answers is registered, so the
    // router admits it. Red before the fix: `bug`, `export`, and `import`
    // answered `unknown command` because they were in the dispatcher and in
    // `/help` but missing from `slash_commands`.
    #[test]
    fn every_handled_command_is_registered() {
        // The three gh #443 casualties specifically, so the regression
        // names them even if someone trims the list later.
        for name in ["bug", "export", "import"] {
            assert!(
                HANDLED_NAMES.contains(&name),
                "/{name} must be registered: it has a dispatch arm and a /help line"
            );
        }
        for name in HANDLED_NAMES {
            assert!(
                !name.is_empty() && !name.starts_with('/'),
                "/{name} is registered without a leading slash"
            );
        }
    }

    // Verifies: gh #443 - a name with no dispatch arm does not get
    // registered, because a listed-but-unhandled name falls through to the
    // extension dispatcher and silently does nothing (the converse bug).
    #[test]
    fn every_registered_name_has_a_dispatch_arm() {
        let arms = dispatch_arms();
        for name in HANDLED_NAMES {
            assert!(
                arms.iter().any(|arm| arm == name),
                "/{name} is registered but the host dispatcher has no arm for it; \
                 a listed-but-unhandled name does nothing silently. Arms seen: {arms:?}"
            );
        }
    }

    // Verifies: gh #443 - the guard's own reader works, so the two rows
    // above cannot pass vacuously on an empty arm list. If the dispatcher
    // is ever rewritten into a shape this scanner cannot read, this fails
    // loudly instead of the guards silently checking nothing.
    #[test]
    fn the_arm_reader_finds_the_dispatch_arms() {
        let arms = dispatch_arms();
        for expected in ["attach", "bug", "export", "import", "login", "usage"] {
            assert!(
                arms.iter().any(|arm| arm == expected),
                "the arm reader missed /{expected}; arms seen: {arms:?}"
            );
        }
    }
}

/// The `/permissions` confirmation line (gh #261).
///
/// This line is the only feedback the user gets, and the old one was ambiguous:
/// `deny rule read muffin-recipe.txt` read as though the whole string were the
/// pattern. It now states the pattern that will actually be matched, and - when
/// compiling changed the text - also what happens for the action kind, so a user
/// can see the promise and the form together.
fn confirm_rule(
    decision_word: &str,
    compiled: &lca_permissions::rule_entry::CompiledRule,
) -> String {
    let scope_note = match compiled.verb {
        None => String::new(),
        Some(verb) => {
            let verb = match verb {
                lca_permissions::rule_entry::RuleVerb::Read => "read",
                lca_permissions::rule_entry::RuleVerb::Write => "write",
            };
            format!(" for {verb} actions")
        }
    };
    if compiled.pattern == compiled.typed {
        // Nothing was rewritten: say it once.
        format!("{decision_word} rule `{}`{scope_note}", compiled.pattern)
    } else {
        // The stored form differs from what was typed: say both, so the user is
        // never surprised later by a pattern they did not literally write.
        format!(
            "{decision_word} rule `{}`{scope_note} (from `{}`)",
            compiled.pattern, compiled.typed
        )
    }
}

#[cfg(test)]
mod rule_entry_tests {
    use super::confirm_rule;

    // Verifies: gh #261 - the confirmation names the pattern that will actually
    // be matched, and says so unambiguously when it differs from what was typed.
    //
    // The message was half the defect: `deny rule read muffin-recipe.txt` read as
    // though the whole string were the pattern. A user who reads that and gets no
    // effect has no way to tell whether the rule or the matcher is wrong.
    #[test]
    fn the_permission_confirmation_names_the_matched_pattern() {
        let cwd = std::path::Path::new("/home/me/proj");
        // A relative path compiles to a spanning glob: both forms must appear.
        let compiled =
            lca_permissions::rule_entry::compile("read muffin-recipe.txt", cwd).expect("compiles");
        let line = confirm_rule("deny", &compiled);
        assert!(
            line.contains("*muffin-recipe.txt"),
            "the line states the pattern that will match: {line}"
        );
        assert!(
            line.contains("from `read muffin-recipe.txt`"),
            "and what the user typed, so the rewrite is not silent: {line}"
        );
        assert!(
            line.contains("for read actions"),
            "and which action kind it is about: {line}"
        );

        // An anchored glob passes through, so there is nothing to explain away.
        let plain =
            lca_permissions::rule_entry::compile("*muffin-recipe.txt", cwd).expect("compiles");
        let line = confirm_rule("deny", &plain);
        assert_eq!(line, "deny rule `*muffin-recipe.txt`", "{line}");

        // A bare shell rule is stated once, with no action-kind note.
        let shell = lca_permissions::rule_entry::compile("git push*", cwd).expect("compiles");
        assert_eq!(confirm_rule("allow", &shell), "allow rule `git push*`");
    }
}
