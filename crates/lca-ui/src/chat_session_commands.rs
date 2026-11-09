//! Session management commands and the in-place switch path: `/tree`,
//! `/fork`, `/resume`, `/rename`/`/name`, and `/clone`/`/jump`'s shared
//! machinery.
//!
//! Split from `chat_commands.rs` at the 1,200-line ceiling (gh #574). The
//! dispatcher stays there; what moves is the *family* it routes to - every
//! method whose subject is a **session** (fork it, branch it, switch it,
//! name it, replay its records) rather than this process's configuration.
//! `switch_or_announce`/`replay_records` sit here because they are how a
//! session lands, not how a command is parsed.
//!
//! The orphaned doc comment that used to sit above `commit_tree_label` (a
//! leftover from an earlier split, describing an `expand_template` that had
//! already moved to `chat_ext_commands.rs`) was dropped rather than carried.

use super::chat::Chat;
use crate::chat_pickers::TreePicker;

impl Chat {
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
        // Gh #400 gap 2: the startup header outlives `/new` and a session switch.
        // `clear()` drops every entry, so the header entry is re-pushed here with
        // its expand state carried across - a header that resets on `/new` is worse
        // than one that never expanded (the issue's own words).
        let expanded = self.transcript.startup_resources_expanded();
        self.transcript.clear();
        if !self.startup_header.0.is_empty() || !self.startup_header.1.is_empty() {
            self.transcript.push_startup_resources(
                self.startup_header.0.clone(),
                self.startup_header.1.clone(),
            );
            if expanded {
                self.transcript.toggle_startup_resources();
            }
        }
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
