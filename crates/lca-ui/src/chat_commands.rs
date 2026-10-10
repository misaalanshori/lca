//! The command language and the session-switch path, split from
//! `chat.rs` (the 1,200-line ceiling).

use std::sync::Arc;

use lca_protocol::CommandEffect;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_tui::engine::keys;
use lca_tui::widgets::autocomplete::{
    ArgumentCompletions, AutocompleteItem, CombinedAutocompleteProvider, SlashCommand,
};

use super::chat::Chat;
use crate::chat_pickers::{
    SettingsPicker, ThemePicker, ThinkingPicker, TreePicker, TrustPicker, thinking_offered_all,
};
use crate::state::{Action, UiOptions};

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
            .map(|offer| offer())
            .filter(|offered| !offered.is_empty())
            .unwrap_or_else(thinking_offered_all);
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
            "mcp" => {
                let Some(act) = self.world.options.hooks.mcp_action.clone() else {
                    self.world.notice =
                        Some("no MCP servers configured (see the authoring guide)".to_string());
                    return Action::Continue;
                };
                let mut words = argument.splitn(2, ' ');
                let verb = words.next().unwrap_or("").to_string();
                let target = words.next().unwrap_or("").trim().to_string();
                self.world.notice = Some(act(&verb, &target));
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
            "hotkeys" => {
                self.push_output(hotkeys_notice(&self.keybindings));
                return Action::Continue;
            }
            "fullscreen" => {
                self.screen_mode = !self.screen_mode;
                if let Some(persist) = &self.world.options.hooks.persist_screen_mode {
                    persist(self.screen_mode);
                }
                self.world.notice = Some(format!(
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
            "resume" => {
                self.open_resume_picker();
                return Action::Continue;
            }
            "grants" => {
                let Some(list) = self.world.options.hooks.grants.as_ref() else {
                    self.world.notice = Some("grants are not available in this host".to_string());
                    return Action::Continue;
                };
                let entries = list();
                if entries.is_empty() {
                    self.world.notice = Some("no grants recorded for this project".to_string());
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
                    self.world.notice =
                        Some("a turn is running; finish or cancel it before reloading".to_string());
                    return Action::Continue;
                }
                let Some(reload) = self.world.options.hooks.reload.clone() else {
                    self.world.notice = Some("reloading is not available in this host".to_string());
                    return Action::Continue;
                };
                let report = reload();
                // gh #233: a rebuilt registry offers a new catalog.
                self.invalidate_models();
                // Interface-owned state refreshes here; the host did
                // the files, the registry, and the agent config.
                self.world.options.themes = report.themes;
                self.keybindings =
                    Arc::new(KeybindingsManager::with_user_bindings(report.key_bindings));
                self.world.options.keybinding_error = report.keybinding_error;
                self.world.notice = Some(report.notice);
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
                        Err(_) => {
                            self.world.notice = Some("usage: /fork <message-index>".to_string())
                        }
                    }
                }
                return Action::Continue;
            }
            // Gh #205: duplicate the tip and switch in one step (no
            // picker). The hook duplicates; the switch path replays.
            "clone" => {
                let Some(clone) = self.world.options.hooks.clone_session.clone() else {
                    self.world.notice = Some("cloning is not available in this host".to_string());
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
                            self.world.notice = Some(format!("cloned session as '{title}' ({id})"));
                        }
                    }
                    Err(err) => self.world.notice = Some(err),
                }
                return Action::Continue;
            }
            // Gh #37: bookmark the nth user message (`/label <n>
            // <name>), defaulting to the latest (`/label <name>`).
            "label" => {
                let Some(set_label) = self.world.options.hooks.set_label.clone() else {
                    self.world.notice = Some("labeling is not available in this host".to_string());
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
                    self.world.notice = Some("usage: /label [message-index] <name>".to_string());
                    return Action::Continue;
                }
                match set_label(index, &name) {
                    Ok(target) => {
                        self.world.notice = Some(format!("bookmarked '{name}' at {target}"));
                    }
                    Err(err) => self.world.notice = Some(err),
                }
                return Action::Continue;
            }
            // Gh #37: list every live bookmark, oldest name first.
            "labels" => {
                let Some(list_labels) = self.world.options.hooks.list_labels.clone() else {
                    self.world.notice = Some("labeling is not available in this host".to_string());
                    return Action::Continue;
                };
                let marks = list_labels();
                if marks.is_empty() {
                    self.world.notice = Some(
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
                    self.world.notice = Some("labeling is not available in this host".to_string());
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
                    self.world.notice = Some(format!("no bookmark named '{name}'"));
                    return Action::Continue;
                };
                let report = fork_record(&target);
                match &report.id {
                    Some(id) => {
                        let running = self.turn_running;
                        self.switch_or_announce(id);
                        if !running {
                            self.world.notice =
                                Some(format!("branched at '{name}' (session: {id})"));
                        }
                    }
                    None => self.world.notice = Some(report.notice),
                }
                return Action::Continue;
            }
            // Gh #37: rename the live session (the entry trails in the
            // log as `session-info`; `/resume` keeps showing the title).
            // Gh #75: pi's `/name` is the same verb under its own name.
            "rename" | "name" => {
                let Some(rename) = self.world.options.hooks.rename_session.clone() else {
                    self.world.notice = Some("renaming is not available in this host".to_string());
                    return Action::Continue;
                };
                let name = argument.trim();
                if name.is_empty() {
                    self.world.notice = Some("usage: /rename <name>".to_string());
                    return Action::Continue;
                }
                match rename(name) {
                    Ok(notice) => self.world.notice = Some(notice),
                    Err(err) => self.world.notice = Some(err),
                }
                return Action::Continue;
            }
            // Gh #204: the quick-cycle checklist - toggles flip rows,
            // enter saves (persists `models.enabled` + live rotation),
            // escape discards.
            "scoped-models" => {
                let Some(list) = self.world.options.hooks.scoped_models.clone() else {
                    self.world.notice =
                        Some("scoped models are not available in this host".to_string());
                    return Action::Continue;
                };
                let rows = list();
                if rows.is_empty() {
                    self.world.notice = Some("no models to scope".to_string());
                } else {
                    self.scoped_models_picker =
                        Some(crate::chat_pickers::ScopedModelsPicker::new(rows));
                }
                return Action::Continue;
            }
            // Gh #131: the embedded changelog's latest released section.
            "changelog" => {
                let section = latest_changelog_section();
                if section.is_empty() {
                    self.world.notice = Some("no released changes yet".to_string());
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
        self.world.notice = Some(crate::state::sanitize_text(&format!(
            "unknown command /{name}"
        )));
        Action::Continue
    }

    /// Expand a `/name args` prompt template into editor text (gh #58):
    /// `None` when no template carries the name.
    /// Run an extension-owned slash command through the host (gh #25
    /// keeps `<provider>.login` here too): the shared tail of
    /// `dispatch_command` and the async model-consent path (gh #232 -
    /// an empty discovery still reaches endpoint consent).
    pub(crate) fn dispatch_extension_command(&mut self, name: &str, argument: &str) -> Action {
        match (self.world.options.invoke_command)(name, argument) {
            CommandEffect::ShowWidget(text) => {
                self.world.notice = Some(crate::state::sanitize_block(&text));
            }
            CommandEffect::AttachImage {
                media_type,
                bytes,
                note,
            } => {
                let info = lca_tui::widgets::image::ImageInfo::new(&media_type, &bytes);
                self.transcript.push_image(info, bytes);
                self.world.notice = Some(crate::state::sanitize_block(&note));
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

    /// Commit a tree label edit (gh #231): the hook bookmarks (or
    /// clears) by record id, then the navigator rebuilds so the mark
    /// shows on the same frame.
    pub(crate) fn commit_tree_label(&mut self, id: &str, name: &str) {
        let Some(label) = self.world.options.hooks.label_record.clone() else {
            self.world.notice = Some("labeling is not available in this host".to_string());
            return;
        };
        match label(id, name) {
            Ok(notice) => {
                self.world.notice = Some(notice);
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
            Err(err) => self.world.notice = Some(err),
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
            self.world.notice = Some("no branches yet".to_string());
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
            self.world.notice = Some("deleting sessions is not available in this host".to_string());
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
                self.world.notice = Some(notice);
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
            Err(err) => self.world.notice = Some(err),
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
            self.world.notice = Some("no sessions yet".to_string());
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
            self.world.notice = Some("No messages to fork from".to_string());
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
            self.world.notice = Some("forking is not available in this host".to_string());
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
                    self.world.notice = Some(report.notice);
                }
            }
            _ => self.world.notice = Some(report.notice),
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
                    true
                } else {
                    false
                }
            }
            crate::state::CompactState::Running => {
                if self.compacting {
                    false
                } else {
                    self.compacting = true;
                    self.separator.working();
                    self.world.notice = Some("compacting this session…".to_string());
                    true
                }
            }
            crate::state::CompactState::Done(notice) => {
                self.compacting = false;
                self.separator.idle();
                self.world.notice = Some(notice);
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
            self.world.notice =
                Some("a turn is running; finish or cancel it before switching".to_string());
            return;
        }
        // gh #233: a switch can change provider readiness.
        self.invalidate_models();
        match self.world.options.hooks.switch_session.as_ref() {
            Some(switch) => match switch(id) {
                Some(records) => {
                    self.replay_records(&records);
                    self.world.notice = Some(format!("switched to session {id}"));
                }
                None => self.world.notice = Some(format!("cannot open session {id}")),
            },
            None => {
                self.world.notice = Some(format!("resume this branch with: lca --resume {id}"));
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
            self.world.notice =
                Some("a turn is running; finish or cancel it before switching".to_string());
            return;
        }
        match self.world.options.hooks.branch_here.as_ref() {
            Some(branch) => match branch(record_id) {
                Some(records) => {
                    self.replay_records(&records);
                    self.world.notice = Some(format!("branched at {record_id}; type to continue"));
                }
                None => self.world.notice = Some(format!("cannot branch at {record_id}")),
            },
            None => self.world.notice = Some("branching is not available in this host".to_string()),
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

/// Build the autocomplete chain: commands, arguments, and file paths.
pub(crate) fn provider_for(options: &UiOptions) -> CombinedAutocompleteProvider {
    let models = options.models.clone();
    let providers: Vec<String> = options
        .slash_commands
        .iter()
        .filter_map(|c| c.strip_prefix('/'))
        .filter_map(|c| c.strip_suffix(".login"))
        .map(str::to_string)
        .collect();
    let mut commands: Vec<SlashCommand> = options
        .slash_commands
        .iter()
        .map(|command| {
            let name = command.strip_prefix('/').unwrap_or(command).to_string();
            let argument_completions: Option<ArgumentCompletions> = match name.as_str() {
                "model" => {
                    let models = models.clone();
                    Some(Arc::new(move |prefix: &str| {
                        models
                            .iter()
                            .filter(|(id, _)| id.starts_with(prefix))
                            .map(|(id, label)| AutocompleteItem {
                                // The insert is the raw id; only the menu
                                // row shows the label (G2).
                                value: id.clone(),
                                label: label.clone(),
                                description: None,
                            })
                            .collect()
                    }))
                }
                "login" => {
                    let providers = providers.clone();
                    Some(Arc::new(move |prefix: &str| {
                        providers
                            .iter()
                            .filter(|p| p.starts_with(prefix))
                            .map(|p| AutocompleteItem {
                                value: p.clone(),
                                label: p.clone(),
                                description: None,
                            })
                            .collect()
                    }))
                }
                _ => None,
            };
            SlashCommand {
                name,
                description: None,
                argument_hint: None,
                argument_completions,
            }
        })
        .collect();
    // Gh #58: templates complete with their description and hint.
    if let Some(templates) = options.hooks.prompt_templates.as_ref() {
        for template in templates() {
            if !commands.iter().any(|c| c.name == template.name) {
                commands.push(SlashCommand {
                    name: template.name,
                    description: (!template.description.is_empty()).then_some(template.description),
                    argument_hint: (!template.argument_hint.is_empty())
                        .then_some(template.argument_hint),
                    argument_completions: None,
                });
            }
        }
    }
    CombinedAutocompleteProvider::new(commands, options.workspace.clone())
}

/// The product changelog, baked in at build time (gh #131): `/changelog`
/// reads it without reaching the network or the filesystem.
const CHANGELOG_TEXT: &str = include_str!("../../../CHANGELOG.md");

/// The latest released section: the first `## [` block that is not
/// `[Unreleased]`, through the next `## [` or the end.
fn latest_changelog_section() -> String {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in CHANGELOG_TEXT.lines() {
        if line.starts_with("## [") {
            if in_section {
                break;
            }
            in_section = !line.contains("[Unreleased]");
            if in_section {
                out.push(line);
            }
            continue;
        }
        if in_section {
            out.push(line);
        }
    }
    out.join("\n").trim().to_string()
}

/// One-line descriptions for the interface's own commands.
fn command_help(command: &str) -> &'static str {
    match command {
        "/help" => "list commands and keys",
        "/hotkeys" => "list every key binding",
        "/fullscreen" => "toggle terminal scrollback and app-owned screen (fullscreen)",
        "/theme" => "pick a theme with live preview",
        "/thinking" => "set the reasoning level",
        "/tree" => "browse the session's entry tree and branch in place",
        "/rename" => "rename this session (usage: /rename <name>)",
        "/name" => "rename this session, pi's spelling (usage: /name <name>)",
        "/resume" => "search and reopen a session",
        "/fork" => "fork a branch at a message (picker, or /fork <n>)",
        "/clone" => "duplicate this session at its tip and switch (usage: /clone [name])",
        "/label" => "bookmark a message (usage: /label [message-index] <name>)",
        "/labels" => "list every bookmark",
        "/jump" => "branch at a bookmark and switch (usage: /jump <name>)",
        "/scoped-models" => "choose the quick-cycle rotation (Ctrl+P)",
        "/changelog" => "show the latest released changes",
        "/reload" => {
            "re-run discovery without restarting (settings, extensions, prompts, themes, keys)"
        }
        "/exit" => "leave the interface",
        "/login" => "sign in to a provider",
        "/logout" => "clear the provider's stored key",
        "/usage" => "show the provider's usage, when it has one",
        "/model" => "list or switch the session's model",
        "/compact" => "summarize the session to free context",
        "/attach" => "attach an image to the next message",
        "/trust" => "trust the project folder (auto-approve in-workspace commands)",
        "/permissions" => "manage allow/deny rules (session, project, global)",
        "/grants" => "review this project's grants and rules",
        "/mcp" => "manage MCP servers (status, reconnect, enable, exposure, login)",
        "/settings" => "show the merged configuration and where each value came from",
        "/session" => "show this session's tokens, cache, and cost",
        _ => "",
    }
}

/// The `/hotkeys` text: the LIVE binding registry prints itself (gh
/// #66), so rebound keys show their effective bindings, not defaults.
fn hotkeys_notice(kb: &KeybindingsManager) -> String {
    let mut lines = vec!["keys:".to_string()];
    for (action, keys) in kb.resolved_bindings() {
        if keys.is_empty() {
            continue;
        }
        let description = kb.description(&action).unwrap_or("");
        lines.push(format!("  {} - {description}", keys.join(", ")));
    }
    // R2: the main screen never captures the mouse; `/fullscreen` does, so
    // the terminal's own bypass is worth naming for the fullscreen case.
    lines.push(
        "  Shift+click (or your terminal's bypass key) selects text in fullscreen".to_string(),
    );
    lines.join("\n")
}

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

    /// One step on a `/settings` row (gh #30): the next value in the
    /// row's list (wrapping), applied and persisted, then the rows are
    /// re-read so value and source show what actually won.
    pub(super) fn cycle_setting(
        &mut self,
        picker: &mut crate::chat_pickers::SettingsPicker,
        forward: bool,
    ) -> String {
        let Some(row) = picker.rows.get(picker.selected) else {
            return String::new();
        };
        if row.values.is_empty() {
            return format!("{} is edited through its picker", row.key);
        }
        let (key, values, current) = (row.key.clone(), row.values.clone(), row.value.clone());
        let position = values
            .iter()
            .position(|value| *value == current)
            .unwrap_or(0);
        let next = if forward {
            (position + 1) % values.len()
        } else {
            (position + values.len() - 1) % values.len()
        };
        let notice = self.apply_setting(&key, &values[next]);
        picker.rows = self.settings_rows();
        picker.selected = picker.selected.min(picker.rows.len().saturating_sub(1));
        notice
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
            chat.world.notice = Some(notice);
            true
        }
        None => false,
    }
}
