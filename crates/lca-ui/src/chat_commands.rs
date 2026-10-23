//! The command language and the session-switch path, split from
//! `chat.rs` (the 1,200-line ceiling).

use std::sync::Arc;

use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::keys;

pub(crate) use crate::command_help_table::command_help;

use super::chat::Chat;
use crate::chat_pickers::{
    SettingsPicker, ThemePicker, ThinkingPicker, TreePicker, TrustPicker, thinking_offered_all,
};
use crate::state::Action;

/// Commands that must wait for the turn boundary (composer-polish
/// fold-in): they touch the session while a running turn may be appending
/// to it, so they queue as pending commands and run when the turn ends
/// instead of racing it. Everything else dispatches at once, even mid-turn.
pub(super) fn is_turn_boundary_command(line: &str) -> bool {
    let name = line
        .strip_prefix('/')
        .unwrap_or(line)
        .split([' ', '\t'])
        .next()
        .unwrap_or("");
    matches!(name, "compact")
}

impl Chat {
    /// Open the theme picker on the current theme (the `/theme` command
    /// and the `/settings` selector's `ui.theme` row both land here).
    pub(super) fn open_theme_picker(&mut self) {
        self.theme_picker = Some(ThemePicker {
            selected: self
                .theme_names
                .iter()
                .position(|name| *name == self.theme_name)
                .unwrap_or(0),
            original: self.theme_name.clone(),
        });
    }

    /// Open the thinking picker on the current level (`/thinking` and
    /// the `/settings` selector's `thinking` row): the host's offered
    /// set for the current model, every level without one (gh #41).
    pub(super) fn open_thinking_picker(&mut self) {
        let current = self.thinking_level();
        let offered = self
            .world
            .options
            .hooks
            .thinking_offered
            .as_ref()
            .map(|offer| offer());
        // Gh #242: an explicitly empty offer means the model does
        // not reason - notice instead of an empty picker. No hook
        // (or nothing configured anywhere) keeps the historical all.
        let Some(offered) = offered else {
            return self.open_thinking_picker_with(thinking_offered_all(), current);
        };
        if offered.is_empty() {
            self.say(format!(
                "Current model ({}) does not support thinking",
                self.model_label()
            ));
            return;
        }
        self.open_thinking_picker_with(offered, current);
    }

    /// Open the **stepped thinking wizard** (gh #397): the model, then its
    /// thinking level.
    ///
    /// This is the issue's example of a group a flat list cannot express. The
    /// second step's options depend on the first, so the wizard computes them from
    /// the *pending* model - not the applied one - or cancelling would leave the
    /// session describing a model it is not on. Nothing is written until the last
    /// step is confirmed, so `Esc` at step two changes nothing at all.
    ///
    /// A model that cannot reason still gets a wizard of one step, because the
    /// wizard's job here is the thinking level; the notice names the limitation
    /// rather than refusing to open.
    pub(super) fn open_thinking_wizard(&mut self) {
        eprintln!("OPEN_THINKING_WIZARD called");
        let current_level = self.thinking_level().unwrap_or_default();
        let offered = self
            .world
            .options
            .hooks
            .thinking_offered
            .as_ref()
            .map(|offer| offer())
            .unwrap_or_else(thinking_offered_all);
        // A model that does not reason still gets the wizard: step one (which
        // model) is a real choice whether or not the current one reasons, and
        // the level step then says why it offers nothing rather than the wizard
        // refusing to open. Refusing here made the row look dead on exactly the
        // configuration where a user would go looking for the model step - found
        // in the `e2e_settings_selector` sandbox, whose mock model declares no
        // levels.
        let model = self.model_label();
        let steps = vec![
            crate::chat_pickers::WizardStep {
                key: "provider.model".to_string(),
                label: "Model".to_string(),
                value: model.clone(),
                // The model step offers the models the session can switch to;
                // without a host list the current model is the only stop, which
                // still lets step two be reached and is honest about what is
                // known.
                values: self.wizard_models(),
            },
            crate::chat_pickers::WizardStep {
                key: "thinking".to_string(),
                label: "Thinking level".to_string(),
                value: current_level,
                values: offered,
            },
        ];
        self.world.wizard = Some(crate::chat_pickers::SteppedWizard::new("thinking", steps));
    }

    /// The models the thinking wizard's first step offers: the host's list when
    /// there is one, else just the current model.
    fn wizard_models(&self) -> Vec<String> {
        let current = self.model_label();
        let mut models = vec![current.clone()];
        // The live list when the host has one, the startup snapshot otherwise -
        // the same two sources `/model` reads, so the wizard cannot offer a
        // different set from the picker beside it.
        let live = self.world.options.hooks.models.as_ref().map(|list| list());
        for (id, _label) in live.unwrap_or_else(|| self.world.options.models.clone()) {
            if !models.contains(&id) {
                models.push(id);
            }
        }
        models
    }

    fn open_thinking_picker_with(&mut self, offered: Vec<String>, current: Option<String>) {
        let selected = match current.as_deref() {
            None => 0,
            Some(level) => offered
                .iter()
                .position(|name| name == level)
                .map_or(0, |index| index + 1),
        };
        self.thinking_picker = Some(ThinkingPicker { selected, offered });
    }

    /// Dispatch a slash command line.
    pub(crate) fn dispatch_command(&mut self, line: &str) -> Action {
        let command_line = line.strip_prefix('/').unwrap_or(line);
        let mut parts = command_line.splitn(2, ' ');
        let name = parts.next().unwrap_or("").to_string();
        let argument = parts.next().unwrap_or("").to_string();
        // gh #43: pi's `/skill:name` colon form routes like the space
        // form (`/skill name`), so both reach the host's `skill` command.
        // Only the `skill` head splits: other commands keep their colons.
        let (name, argument) = match name.split_once(':') {
            Some(("skill", rest)) => {
                let argument = if argument.is_empty() {
                    rest.to_string()
                } else {
                    format!("{rest} {argument}")
                };
                ("skill".to_string(), argument)
            }
            _ => (name, argument),
        };
        match name.as_str() {
            // gh #53: the MCP manager - verbs, no picker. The status
            // block lists the verbs; each answer lands as a notice.
            // Shell management waits for #171 (0.7); files stay
            // hand-editable, and `/reload` re-reads them.
            // Gh #418 Phase G: bare `/mcp` (and `/mcp list`) opens the manager
            // modal; every other verb keeps its direct route. The modal is an
            // addition, not a replacement - `/mcp login echo` and friends are
            // the scriptable form and must keep answering with a notice, which
            // is what `mcp_verbs_route_through_the_hooks` still pins.
            //
            // No match guard on the arm: the `/help` guard reads arms textually,
            // and a guard's own string literal would be read as a command name.
            // `/mcp list` is handled inside the verb arm below.
            "mcp" if argument.trim().is_empty() => {
                return self.open_mcp_manager();
            }
            "mcp" => {
                // `/mcp list` is the same screen as bare `/mcp`: a user who
                // reached for a verb name gets the manager rather than an
                // "unknown verb" refusal, and the direct verbs keep their route.
                if argument.trim() == "list" {
                    return self.open_mcp_manager();
                }
                let Some(act) = self.world.options.hooks.mcp_action.clone() else {
                    self.say("no MCP servers configured (see the authoring guide)".to_string());
                    return Action::Continue;
                };
                let mut words = argument.splitn(2, ' ');
                let verb = words.next().unwrap_or("").to_string();
                let target = words.next().unwrap_or("").trim().to_string();
                self.say(act(&verb, &target));
                return Action::Continue;
            }
            "help" => {
                // Gh #58: template commands list with their descriptions.
                let mut commands = self.world.options.slash_commands.clone();
                if let Some(templates) = self.world.options.hooks.prompt_templates.as_ref() {
                    for template in templates() {
                        commands.push(format!("/{}", template.name));
                    }
                }
                self.push_output(help_notice(&commands));
                return Action::Continue;
            }
            // Gh #411 gaps 2 and 3: `/bug` stages a **scrubbed copy**, then shows
            // the inspection screen. The zip is written only when the user
            // confirms, and cancelling removes the staging directory. The host
            // owns the staging and the zip; this side owns the screen and the key.
            "bug" if self.world.options.hooks.stage_bug.is_some() => {
                // The gate. A host with `stage_bug` can stage and write, so the
                // inspection screen takes over; a host without it falls through to
                // the registry below, which is how a non-interactive host still
                // gets its one-shot bundle.
                if let Some(stage) = self.world.options.hooks.stage_bug.clone() {
                    match stage() {
                        Ok(review) => self.world.bug_review = Some(review),
                        Err(err) => self.say(crate::state::sanitize_block(&err)),
                    }
                }
                return Action::Continue;
            }
            // Gh #412: `/persona` is the bundle, not a new interaction. With no
            // argument it lists what each persona carries; with a name it validates
            // the **whole** bundle against this session and applies only if every
            // field is reachable. A half-applied persona is worse than a refusal,
            // because the user believes they are in a persona they are not in.
            "persona" => {
                self.dispatch_persona(&argument);
                return Action::Continue;
            }
            "hotkeys" => {
                self.push_output(crate::chat_notices::hotkeys_notice(&self.keybindings));
                return Action::Continue;
            }
            "fullscreen" => {
                self.screen_mode = !self.screen_mode;
                if let Some(persist) = &self.world.options.hooks.persist_screen_mode {
                    persist(self.screen_mode);
                }
                self.say(format!(
                    "screen mode: {}",
                    if self.screen_mode {
                        "app-owned screen (fullscreen)"
                    } else {
                        "terminal scrollback"
                    }
                ));
                return Action::Continue;
            }
            "theme" => {
                self.open_theme_picker();
                return Action::Continue;
            }
            "thinking" => {
                self.open_thinking_picker();
                return Action::Continue;
            }
            "trust" => {
                self.trust_picker = Some(TrustPicker { selected: 0 });
                return Action::Continue;
            }
            // gh #30 (EFG-030): the interactive selector, pi's shape on
            // our picker chrome. The guard keeps the host's read-only
            // dump reachable for a host that ships no rows (FR-CFG-2's
            // command half).
            "settings" if self.world.options.hooks.settings_rows.is_some() => {
                let rows = self.settings_rows();
                self.settings_picker = Some(SettingsPicker {
                    rows,
                    selected: 0,
                    editing: None,
                    query: String::new(),
                    remembered: std::collections::BTreeMap::new(),
                });
                return Action::Continue;
            }
            // gh #233: the catalog enumerates lazily here only (every
            // other command dispatches without touching providers), and
            // the snapshot behind it invalidates on login, switch,
            // grant, reload, and scope events. An empty list falls
            // through to the extension `model` command below, which
            // runs endpoint consent (gh #31) instead of reporting
            // nothing - short-circuiting here would amputate that.
            "model" if argument.trim().is_empty() => {
                // gh #232: instant open, never a blocking enumerate -
                // the snapshot answers or a loader shows while the
                // background thread runs. An empty discovery closes the
                // loader and falls through to consent below.
                self.open_model_picker();
                if self.model_picker.is_some() {
                    return Action::Continue;
                }
            }
            "tree" => {
                self.open_tree_picker();
                return Action::Continue;
            }
            // #227: the Extension Hub opens like the model picker -
            // host rows through hooks, never a blocking fetch here.
            "extensions" | "marketplace" => {
                self.dispatch_extensions();
                return Action::Continue;
            }
            // gh #3: the extension manager (enable/disable plus the
            // compaction-strategy picker) - the Hub discovers.
            "ext" => {
                self.dispatch_ext();
                return Action::Continue;
            }
            "resume" => {
                self.open_resume_picker();
                return Action::Continue;
            }
            "grants" => {
                let Some(list) = self.world.options.hooks.grants.as_ref() else {
                    self.say("grants are not available in this host".to_string());
                    return Action::Continue;
                };
                let entries = list();
                if entries.is_empty() {
                    self.say("no grants recorded for this project".to_string());
                } else {
                    self.grants_picker = Some(crate::chat_pickers::GrantPicker {
                        entries,
                        selected: 0,
                    });
                }
                return Action::Continue;
            }
            // Gh #130: re-run discovery without restarting. Refused
            // mid-turn like a session switch: a reload must not land
            // between a turn's declare and its dispatch.
            "reload" => {
                if self.turn_running {
                    self.say("a turn is running; finish or cancel it before reloading".to_string());
                    return Action::Continue;
                }
                let Some(reload) = self.world.options.hooks.reload.clone() else {
                    self.say("reloading is not available in this host".to_string());
                    return Action::Continue;
                };
                let report = reload();
                // gh #233: a rebuilt registry offers a new catalog.
                self.invalidate_models();
                // Interface-owned state refreshes here; the host did
                // the files, the registry, and the agent config.
                self.world.options.themes = report.themes;
                // Gh #387 gap 5: re-apply the **active** theme, not just refresh
                // the picker's list. The host hook re-reads the theme directory
                // and reports the names it found; the palette on screen is
                // interface state, so an edited `<name>.toml` only took effect on
                // restart while this notice claimed "themes" had been reloaded.
                // Found by this batch's live verification: set one parent role,
                // edit it, `/reload`, and nothing on screen changed.
                let setting = self.theme_setting().to_string();
                if !setting.is_empty() && !self.theme_follows_detection() {
                    self.set_theme(&setting);
                }
                // Gh #410: a rebind has to reach the labels, not just the key
                // handler. `set_keybindings` keeps the transcript's hint manager
                // in step; assigning the field alone left every rendered hint on
                // the manager it was built with, so `/reload` changed which keys
                // *worked* while the hints kept naming the old ones. Found by
                // driving a real pane: the tool card still said `ctrl+o` after
                // reloading a `ctrl+e` binding.
                self.set_keybindings(Arc::new(KeybindingsManager::with_user_bindings(
                    report.key_bindings,
                )));
                self.world.options.keybinding_error = report.keybinding_error;
                self.say(report.notice);
                return Action::Continue;
            }
            // Gh #203: bare `/fork` opens the user-message picker;
            // `/fork <n>` forks directly. Both switch in-process with
            // the message text restored into the editor.
            "fork" => {
                if argument.trim().is_empty() {
                    self.open_fork_picker();
                } else {
                    match argument.trim().parse::<usize>() {
                        Ok(index) => self.fork_and_switch(index),
                        Err(_) => self.say("usage: /fork <message-index>".to_string()),
                    }
                }
                return Action::Continue;
            }
            // Gh #205: duplicate the tip and switch in one step (no
            // picker). The hook duplicates; the switch path replays.
            "clone" => {
                let Some(clone) = self.world.options.hooks.clone_session.clone() else {
                    self.say("cloning is not available in this host".to_string());
                    return Action::Continue;
                };
                let name = argument.trim();
                let name = (!name.is_empty()).then(|| name.to_string());
                match clone(name) {
                    Ok((id, title)) => {
                        // A running turn refuses the switch inside
                        // `switch_or_announce` (its notice stands); the
                        // clone itself still landed.
                        let running = self.turn_running;
                        self.switch_or_announce(&id);
                        if !running {
                            self.say(format!("cloned session as '{title}' ({id})"));
                        }
                    }
                    Err(err) => self.say(err),
                }
                return Action::Continue;
            }
            // Gh #37: bookmark the nth user message (`/label <n>
            // <name>), defaulting to the latest (`/label <name>`).
            "label" => {
                let Some(set_label) = self.world.options.hooks.set_label.clone() else {
                    self.say("labeling is not available in this host".to_string());
                    return Action::Continue;
                };
                let users = self
                    .transcript
                    .entries()
                    .iter()
                    .filter(|entry| matches!(entry, crate::transcript::Entry::User(_)))
                    .count();
                // An explicit index plus a name, or just a name for the
                // latest message: the host counts the same nth user
                // record as `/fork` (FR-SESS-10).
                let (index, name) = match argument.trim().split_once(char::is_whitespace) {
                    Some((head, tail)) => match head.parse::<usize>() {
                        Ok(index) => (index, tail.trim().to_string()),
                        Err(_) => (users.saturating_sub(1), argument.trim().to_string()),
                    },
                    None => (users.saturating_sub(1), argument.trim().to_string()),
                };
                if users == 0 || name.is_empty() {
                    self.say("usage: /label [message-index] <name>".to_string());
                    return Action::Continue;
                }
                match set_label(index, &name) {
                    Ok(target) => {
                        self.say(format!("bookmarked '{name}' at {target}"));
                    }
                    Err(err) => self.say(err),
                }
                return Action::Continue;
            }
            // Gh #37: list every live bookmark, oldest name first.
            "labels" => {
                let Some(list_labels) = self.world.options.hooks.list_labels.clone() else {
                    self.say("labeling is not available in this host".to_string());
                    return Action::Continue;
                };
                let marks = list_labels();
                if marks.is_empty() {
                    self.say(
                        "no bookmarks yet; /label <name> marks the latest message".to_string(),
                    );
                } else {
                    self.push_output(
                        marks
                            .iter()
                            .map(|(name, target)| format!("{name} -> {target}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
                return Action::Continue;
            }
            // Gh #37: branch at a bookmark and switch (ADR-0046:
            // navigation is fork-plus-switch until the tree phase).
            "jump" => {
                let (Some(list_labels), Some(fork_record)) = (
                    self.world.options.hooks.list_labels.clone(),
                    self.world.options.hooks.fork_record.clone(),
                ) else {
                    self.say("labeling is not available in this host".to_string());
                    return Action::Continue;
                };
                let name = argument.trim();
                // First match on the name-sorted listing: deterministic.
                // True latest-wins lives in `store.resolve_label`; two
                // live targets sharing a name is pathological.
                let target = list_labels()
                    .into_iter()
                    .find_map(|(mark, target)| (mark == name).then_some(target));
                let Some(target) = target else {
                    self.say(format!("no bookmark named '{name}'"));
                    return Action::Continue;
                };
                let report = fork_record(&target);
                match &report.id {
                    Some(id) => {
                        let running = self.turn_running;
                        self.switch_or_announce(id);
                        if !running {
                            self.say(format!("branched at '{name}' (session: {id})"));
                        }
                    }
                    None => self.say(report.notice),
                }
                return Action::Continue;
            }
            // Gh #37: rename the live session (the entry trails in the
            // log as `session-info`; `/resume` keeps showing the title).
            // Gh #75: pi's `/name` is the same verb under its own name.
            "rename" | "name" => {
                let Some(rename) = self.world.options.hooks.rename_session.clone() else {
                    self.say("renaming is not available in this host".to_string());
                    return Action::Continue;
                };
                let name = argument.trim();
                if name.is_empty() {
                    self.say("usage: /rename <name>".to_string());
                    return Action::Continue;
                }
                match rename(name) {
                    Ok(notice) => self.say(notice),
                    Err(err) => self.say(err),
                }
                return Action::Continue;
            }
            // Gh #204: the quick-cycle checklist - toggles flip rows,
            // enter saves (persists `models.enabled` + live rotation),
            // escape discards.
            "scoped-models" => {
                let Some(list) = self.world.options.hooks.scoped_models.clone() else {
                    self.say("scoped models are not available in this host".to_string());
                    return Action::Continue;
                };
                let rows = list();
                if rows.is_empty() {
                    self.say("no models to scope".to_string());
                } else {
                    self.scoped_models_picker =
                        Some(crate::chat_pickers::ScopedModelsPicker::new(rows));
                }
                return Action::Continue;
            }
            // Gh #131: the embedded changelog's latest released section.
            "changelog" => {
                let section = crate::chat_notices::latest_changelog_section();
                if section.is_empty() {
                    self.say("no released changes yet".to_string());
                } else {
                    self.push_output(section);
                }
                return Action::Continue;
            }
            "quit" | "exit" => return Action::Exit,
            _ => {}
        }

        // gh #25: only the bare `/login` belongs to the options seam. A
        // namespaced `<provider>.login` is that provider's identity `login`
        // export (FR-PROV-10), which the host's command dispatch below owns -
        // intercepting it here sent every provider through the preset picker,
        // including the ones that have no presets to show.
        if name == "login"
            && let Some(login) = self.world.options.login.clone()
        {
            let next = login(argument.as_str());
            self.apply_login_next(next);
            return Action::Continue;
        }

        let full = format!("/{name}");
        if self
            .world
            .options
            .slash_commands
            .iter()
            .any(|command| command == &full)
        {
            return self.dispatch_extension_command(&name, &argument);
        }
        // Gh #58: prompt templates fill the editor (reviewable, never
        // auto-submitted). Extensions keep precedence: their names
        // resolve through invoke_command above.
        if let Some(expanded) = self.expand_template(&name, &argument) {
            self.editor.insert_str(&expanded);
            return Action::Continue;
        }
        self.say(crate::state::sanitize_text(&format!(
            "unknown command /{name}"
        )));
        Action::Continue
    }

    /// Expand a `/name args` prompt template into editor text (gh #58):
    /// `None` when no template carries the name.
    /// Commit a tree label edit (gh #231): the hook bookmarks (or
    /// clears) by record id, then the navigator rebuilds so the mark
    /// shows on the same frame.
    pub(crate) fn commit_tree_label(&mut self, id: &str, name: &str) {
        let Some(label) = self.world.options.hooks.label_record.clone() else {
            self.say("labeling is not available in this host".to_string());
            return;
        };
        match label(id, name) {
            Ok(notice) => {
                self.say(notice);
                let rows = self
                    .world
                    .options
                    .hooks
                    .session_tree
                    .as_ref()
                    .map(|tree| tree())
                    .unwrap_or_default();
                if rows.is_empty() {
                    self.tree_picker = None;
                } else if let Some(picker) = self.tree_picker.as_mut() {
                    let selected = picker.selected_id();
                    picker.rows = rows;
                    picker.rebuild();
                    picker.restore(selected);
                }
            }
            Err(err) => self.say(err),
        }
    }

    /// Open the `/tree` DAG navigator (FR-UI-16, gh #132, gh #231):
    /// an empty list names it instead of opening. The filter opens on
    /// `ui.tree_filter_mode` (active now, not inert).
    pub fn open_tree_picker(&mut self) {
        let rows = self
            .world
            .options
            .hooks
            .session_tree
            .as_ref()
            .map(|tree| tree())
            .unwrap_or_default();
        if rows.is_empty() {
            self.say("no branches yet".to_string());
        } else {
            let mode = self
                .world
                .options
                .hooks
                .tree_filter_mode
                .as_ref()
                .map(|read| read())
                .unwrap_or_default();
            self.tree_picker = Some(TreePicker::new(
                rows,
                crate::chat_pickers::TreeFilter::parse(&mode),
            ));
        }
    }

    /// Pi's double-escape window in milliseconds.
    pub(crate) const DOUBLE_ESCAPE_MS: u64 = 500;

    /// Act on a lone Escape with an empty editor (gh #132): the second
    /// one inside the window opens the tree or the fork picker per the
    /// host's action (`tree` when the host says nothing, pi's default);
    /// `none` never acts. Either way the window re-arms.
    pub(crate) fn double_escape(&mut self) -> Action {
        let armed = self
            .last_escape
            .is_some_and(|at| at.elapsed().as_millis() < Self::DOUBLE_ESCAPE_MS as u128);
        self.last_escape = Some(std::time::Instant::now());
        if !armed {
            return Action::Continue;
        }
        self.last_escape = None;
        let action = self
            .world
            .options
            .hooks
            .double_escape_action
            .as_ref()
            .map(|get| get())
            .unwrap_or_else(|| "tree".to_string());
        match action.as_str() {
            "fork" => self.open_fork_picker(),
            "none" => {}
            _ => self.open_tree_picker(),
        }
        Action::Continue
    }

    /// Commit a resume delete confirm (gh #75): the hook trashes by
    /// record id, then the list rebuilds from the host so the row is
    /// really gone (not just hidden).
    pub(crate) fn commit_resume_delete(&mut self, at: Option<usize>) {
        let Some(delete) = self.world.options.hooks.delete_session.clone() else {
            self.say("deleting sessions is not available in this host".to_string());
            return;
        };
        let id = at.and_then(|selected| {
            let picker = self.resume_picker.as_ref()?;
            let index = picker.matches.get(selected)?;
            picker.entries.get(*index).map(|entry| entry.id.clone())
        });
        let Some(id) = id else {
            return;
        };
        match delete(&id) {
            Ok(notice) => {
                self.say(notice);
                if let Some(list) = self.world.options.hooks.session_list.clone() {
                    let rows = list();
                    if let Some(picker) = self.resume_picker.as_mut() {
                        picker.entries = rows;
                        picker.refilter();
                        picker.selected =
                            picker.selected.min(picker.matches.len().saturating_sub(1));
                    }
                }
            }
            Err(err) => self.say(err),
        }
    }

    /// Open the session picker over the current session (gh #110:
    /// bare `-r` opens here at startup; `/resume` opens here on demand).
    pub fn open_resume_picker(&mut self) {
        let entries = self
            .world
            .options
            .hooks
            .session_list
            .as_ref()
            .map(|list| list())
            .unwrap_or_default();
        if entries.is_empty() {
            self.say("no sessions yet".to_string());
        } else {
            self.resume_picker = Some(crate::resume::ResumePicker::new(entries));
        }
    }

    /// Open the `/fork` user-message picker (gh #203): the transcript's
    /// user messages, latest selected. An empty transcript names it.
    pub fn open_fork_picker(&mut self) {
        let messages: Vec<String> = self
            .transcript
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                crate::transcript::Entry::User(text) => Some(text.clone()),
                _ => None,
            })
            .collect();
        if messages.is_empty() {
            self.say("No messages to fork from".to_string());
        } else {
            self.fork_picker = Some(crate::chat_pickers::ForkPicker::new(messages));
        }
    }

    /// Fork at the nth user message and switch in-process (gh #203): the
    /// fork always lands; a running turn refuses the switch inside
    /// `switch_or_announce` (its notice stands); on a live switch the
    /// message text restores into the editor for immediate re-prompting.
    /// The transcript's user order is the fork's index space (the host
    /// counts the same nth user record); the text is read before the
    /// switch replays the fork over the transcript.
    pub(crate) fn fork_and_switch(&mut self, index: usize) {
        let Some(fork_at) = self.world.options.hooks.fork_at.clone() else {
            self.say("forking is not available in this host".to_string());
            return;
        };
        let text = self
            .transcript
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                crate::transcript::Entry::User(text) => Some(text.clone()),
                _ => None,
            })
            .nth(index);
        let report = fork_at(index);
        match (&report.id, text) {
            (Some(id), Some(text)) => {
                let running = self.turn_running;
                self.switch_or_announce(id);
                if !running {
                    // The host's notice names the fork; the editor
                    // carries the message for immediate re-prompting.
                    self.editor.set_text(&text);
                    self.say(report.notice);
                }
            }
            _ => self.say(report.notice),
        }
    }

    /// Poll a background `/compact` (the command summarizes on its own
    /// thread, so the interface never freezes for it - the failure this
    /// replaced typed a character and only saw it appear 3.5 s later).
    /// `Running` raises the working state and says so; `Done` posts the
    /// summary or the refusal and rests it (chrome.md's compaction
    /// indicator, in LCA's generic working state).
    ///
    /// Returns `true` when something on screen changed.
    pub fn poll_compact(&mut self) -> bool {
        let Some(poll) = self.world.options.hooks.poll_compact.clone() else {
            return false;
        };
        match poll() {
            crate::state::CompactState::Idle => {
                if self.compacting {
                    self.compacting = false;
                    self.separator.idle();
                    // Gh #401: a compaction that ended without a `Done`
                    // handoff (a host that only raises/lowers the state)
                    // still owes the queue its flush.
                    self.flush_compaction_queue();
                    true
                } else {
                    false
                }
            }
            crate::state::CompactState::Running(reason) => {
                if self.compacting {
                    false
                } else {
                    self.compacting = true;
                    // Gh #388 gap 2: a compaction is a first-class separator
                    // state, not "working with a notice elsewhere". The
                    // reason rides through from the host so a threshold cut
                    // the user did not ask for says so.
                    self.separator.compacting(reason);
                    true
                }
            }
            crate::state::CompactState::Done(notice) => {
                self.compacting = false;
                self.separator.idle();
                self.say(notice);
                // Gh #401 (pi's `flushCompactionQueue`): what the user
                // typed while it ran now goes where it belongs.
                self.flush_compaction_queue();
                true
            }
        }
    }

    /// Route an informational multi-line output to the transcript (gh
    /// #234): the dock is a 1-2 line anchor, never a document viewer -
    /// unbounded outputs (`/help`, `/hotkeys`, `/changelog`, a bookmark
    /// list) scroll as transcript entries pi's way, while short
    /// confirmations and errors stay dock notices.
    pub(crate) fn push_output(&mut self, text: String) {
        self.transcript.push_notice(text);
    }

    /// Switch to a session in place when the host supports it (R3), else
    /// print the resume command. Refused while a turn runs: the agent and
    /// its stream must not be swapped mid-flight.
    pub(crate) fn switch_or_announce(&mut self, id: &str) {
        if self.turn_running {
            self.say("a turn is running; finish or cancel it before switching".to_string());
            return;
        }
        // gh #233: a switch can change provider readiness.
        self.invalidate_models();
        match self.world.options.hooks.switch_session.as_ref() {
            Some(switch) => match switch(id) {
                Some(records) => {
                    self.replay_records(&records);
                    self.say(format!("switched to session {id}"));
                }
                None => self.say(format!("cannot open session {id}")),
            },
            None => {
                self.say(format!("resume this branch with: lca --resume {id}"));
            }
        }
    }

    /// Replay records with the live rendering (FR-UI-7), not as plain
    /// lines: the same transcript machinery as a live turn, shared by
    /// session switches and in-place branches.
    pub(crate) fn replay_records(&mut self, records: &[lca_protocol::Record]) {
        self.transcript.clear();
        self.pending.clear();
        let loader = self.world.options.hooks.load_attachment.clone();
        self.load_records(records, loader.as_ref());
    }

    /// Branch at a tree row and replay the new chain in place (gh #37,
    /// FR-UI-16): the log keeps one file, the transcript shows the
    /// branch. Refused while a turn runs, like a session switch.
    pub(crate) fn branch_and_replay(&mut self, record_id: &str) {
        if self.turn_running {
            self.say("a turn is running; finish or cancel it before switching".to_string());
            return;
        }
        match self.world.options.hooks.branch_here.as_ref() {
            Some(branch) => match branch(record_id) {
                Some(records) => {
                    self.replay_records(&records);
                    self.say(format!("branched at {record_id}; type to continue"));
                }
                None => self.say(format!("cannot branch at {record_id}")),
            },
            None => self.say("branching is not available in this host".to_string()),
        }
    }
}

/// Decode the printable text a key inserts, if any.
pub(crate) fn printable(data: &str) -> Option<String> {
    if let Some(text) = keys::decode_printable_key(data) {
        return Some(text);
    }
    match keys::parse_key(data) {
        Some(key) if key.chars().count() == 1 => Some(key),
        Some(key) if key == "space" => Some(" ".to_string()),
        _ => None,
    }
}

/// The flattened text of a bracketed paste, if `data` is one (R1). Every
/// single-line surface — the masked secret field, the base-URL and
/// model-id fields, the picker search boxes, the transcript search —
/// routes through this, so a paste behaves the same everywhere and a
/// multi-line paste is flattened rather than dropped.
pub(crate) fn paste_text(data: &str) -> Option<String> {
    lca_tui::widgets::paste::bracketed_paste_content(data).map(lca_tui::widgets::paste::flatten)
}

pub(crate) use crate::autocomplete_provider::provider_for;

/// The product changelog, baked in at build time (gh #131): `/changelog`
/// reads it without reaching the network or the filesystem.
pub(crate) const CHANGELOG_TEXT: &str = include_str!("../../../CHANGELOG.md");

/// The latest released section: the first `## [` block that is not
/// `[Unreleased]`, through the next `## [` or the end.
/// The `/help` text: the commands the interface offers, then the keys.
fn help_notice(commands: &[String]) -> String {
    let mut sorted: Vec<&String> = commands.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut out = String::from("commands:\n");
    for command in sorted {
        out.push_str("  ");
        out.push_str(command);
        let help = command_help(command);
        if !help.is_empty() {
            out.push_str(" - ");
            out.push_str(help);
        }
        out.push('\n');
    }
    out.push_str("Enter sends, Shift+Enter adds a line, Tab completes, Ctrl+C cancels");
    out
}

impl Chat {
    /// The `/settings` rows (gh #30): the host's hook - key, value,
    /// winning source, cycle values - with the three session-owned
    /// values mirrored from where they actually live (the theme, the
    /// thinking level, the transcript's thinking visibility), the same
    /// live-over-file rule `settings_text` had.
    pub(super) fn settings_rows(&self) -> Vec<crate::state::SettingRow> {
        let rows = self
            .world
            .options
            .hooks
            .settings_rows
            .as_ref()
            .map(|rows| rows())
            .unwrap_or_default();
        rows.into_iter()
            .map(|mut row| {
                match row.key.as_str() {
                    "ui.theme" => row.value = self.theme_name.clone(),
                    "thinking" => {
                        row.value = self
                            .thinking_level()
                            .unwrap_or_else(|| "provider default".to_string());
                    }
                    "ui.thinking" => {
                        row.value = match self.transcript.thinking_visibility() {
                            crate::transcript::ThinkingVisibility::Snippet => "snippet",
                            crate::transcript::ThinkingVisibility::Full => "full",
                            crate::transcript::ThinkingVisibility::Hidden => "hidden",
                        }
                        .to_string();
                    }
                    _ => {}
                }
                row
            })
            .collect()
    }

    /// Re-read the `/settings` rows (gh #30): a sub-picker's write
    /// (theme, thinking) lands outside the cycle path, so the selector
    /// restored underneath would otherwise show the value it had before
    /// it stepped aside. Value and source both come from the fresh read.
    pub(super) fn refresh_settings_rows(&mut self) {
        let rows = self.settings_rows();
        if let Some(picker) = self.settings_picker.as_mut() {
            picker.rows = rows;
            picker.selected = picker.selected.min(picker.rows.len().saturating_sub(1));
        }
    }

    /// One `/settings` edit (gh #30): apply it to the running session
    /// where a seam exists, persist it through the one seam
    /// (`persist_setting_at`, QA-015), and return the notice - honest
    /// about what did not happen: `ui.color` and `shell.tool` resolve at
    /// startup, and `ui.fullscreen` lives in `ui.json`, not the config
    /// file (which is also where this writes it).
    pub(super) fn apply_setting(&mut self, key: &str, value: &str) -> String {
        let persisted = Some(value.to_string());
        match key {
            "ui.thinking" => {
                if let Some(visibility) = crate::transcript::ThinkingVisibility::parse(value) {
                    self.transcript.set_thinking_visibility(visibility);
                }
                if let Some(persist) = &self.world.options.hooks.persist_setting {
                    persist(key, persisted);
                }
                format!("ui.thinking = {value}")
            }
            "thinking" => {
                *self
                    .world
                    .options
                    .thinking
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value.to_string());
                if let Some(persist) = &self.world.options.hooks.persist_setting {
                    persist(key, persisted);
                }
                format!("thinking: {value}")
            }
            "ui.fullscreen" => {
                let on = value.parse::<bool>().unwrap_or(false);
                self.screen_mode = on;
                // The runtime choice's own file (FR-UI-21): `ui.json`,
                // written by the same seam `/fullscreen` uses.
                if let Some(persist) = &self.world.options.hooks.persist_screen_mode {
                    persist(on);
                }
                format!(
                    "fullscreen {} (stored in ui.json)",
                    if on { "on" } else { "off" }
                )
            }
            "permissions.mode" => {
                // ADR-0042: the mode is grant-store state as much as
                // config state - the host's persist hook applies it to
                // the store, the footer marker follows here.
                let yolo = value == "yolo";
                self.world.options.yolo = yolo;
                self.footer.yolo = yolo;
                if let Some(persist) = &self.world.options.hooks.persist_setting {
                    persist(key, persisted);
                }
                if yolo {
                    "permissions.mode = yolo - every prompt is auto-approved and \
                     recorded, explicit deny rules still deny"
                        .to_string()
                } else {
                    "permissions.mode = ask - prompts are asked again".to_string()
                }
            }
            "ui.color" | "shell.tool" => {
                if let Some(persist) = &self.world.options.hooks.persist_setting {
                    persist(key, persisted);
                }
                format!("{key} = {value} (applies to the next session)")
            }
            other => {
                if let Some(persist) = &self.world.options.hooks.persist_setting {
                    persist(other, Some(value.to_string()));
                }
                format!("{other} = {value}")
            }
        }
    }
}

/// Display tunables (gh #82), split from `chat.rs` (the 1,200-line ceiling).
impl Chat {
    /// The live display tunables (gh #82): the host's hook, or the
    /// product defaults when it says nothing.
    pub(crate) fn display_tuning(&self) -> crate::state::DisplayTuning {
        self.world
            .options
            .hooks
            .display_tuning
            .as_ref()
            .map(|tuning| tuning())
            .unwrap_or_default()
    }

    /// Sync the tunables into the editor and transcript (gh #82): the
    /// tick calls this every frame, so a `/settings` cycle applies
    /// without a restart.
    pub(super) fn sync_display_tuning(&mut self) {
        let tuning = self.display_tuning();
        self.editor.max_visible = tuning.autocomplete_max_visible.clamp(3, 20) as usize;
        self.editor.padding_x = tuning.editor_padding_x.min(3) as usize;
        self.transcript.apply_display(&tuning);
    }
}

/// Poll a background `/mcp login` (gh #53): a finished sign-in posts
/// its notice once, like the login poll. Split from `run.rs` (the
/// 1,200-line ceiling); the loop calls it every frame.
pub(crate) fn poll_mcp(chat: &mut Chat) -> bool {
    let Some(poll) = chat.world.options.hooks.poll_mcp.clone() else {
        return false;
    };
    match poll() {
        Some(notice) => {
            chat.say(notice);
            true
        }
        None => false,
    }
}
