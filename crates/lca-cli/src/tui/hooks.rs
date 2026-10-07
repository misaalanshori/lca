//! Host hooks (S1): the `!`/`!!` shell runner, the external editor,
//! screen-mode persistence, the session tree/list/switch/fork surfaces,
//! and the clipboard/URL openers. Each was a closure capturing `run`'s
//! locals; each is now a method on [`Ui`].

use std::sync::{Arc, Mutex};

use lca_protocol::Record;
use lca_session::ViewMode;
use lca_ui::{ShellEvent, ShellHandle, UiHooks};

use super::Ui;
use super::collect_markdown_transformers;
use super::display::age_label;

/// Write text to the system clipboard through a native command, returning
/// `true` only when the command succeeded (R6). A `false` lets the
/// interface fall back to OSC 52 and report it unverified.
fn native_clipboard(text: &str) -> bool {
    #[cfg(target_os = "macos")]
    let candidates: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(target_os = "windows")]
    let candidates: &[(&str, &[&str])] = &[("clip", &[])];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let candidates: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (program, args) in candidates {
        let Ok(mut child) = std::process::Command::new(program)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write as _;
            if stdin.write_all(text.as_bytes()).is_err() {
                let _ = child.kill();
                continue;
            }
        }
        if child.wait().map(|status| status.success()).unwrap_or(false) {
            return true;
        }
    }
    false
}

/// Open a URL in the platform's browser (R6/S7, R3). Fire-and-forget:
/// returns `Err` with the reason when no launcher is available or every
/// candidate refused to start, so the notice can say why rather than just
/// "cannot". The implementation is shared with the OAuth flow
/// (`lca_tools::open_url`): no shell in the path, so a `&`-laden authorize
/// URL cannot be split by a command-line parser.
fn open_url(url: &str) -> Result<(), String> {
    lca_tools::open_url(url)
}

/// Read a child pipe into the shell sink until it closes (R4). Control
/// characters are sanitized so the card can never paint the terminal.
fn read_into<R: std::io::Read>(
    mut reader: Option<R>,
    sink: &std::sync::mpsc::SyncSender<ShellEvent>,
) {
    let Some(reader) = reader.as_mut() else {
        return;
    };
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let text = lca_ui::sanitize_block(&String::from_utf8_lossy(&buf[..n]));
                if sink.send(ShellEvent::Chunk(text)).is_err() {
                    break;
                }
            }
        }
    }
}

impl Ui {
    /// The host hooks wired for this composition (S1).
    pub(super) fn hooks(self: &Arc<Self>) -> UiHooks {
        // E2: keep the live theme cell in step with the persisted pick so
        // `/settings` shows the session value, like it does for thinking.
        let theme_cell = self.theme_cell.clone();
        let grants = self.grants.clone();
        let persist_setting: lca_ui::state::SettingPersist =
            Arc::new(move |key: &str, value: Option<String>| {
                if key == "ui.theme" {
                    *theme_cell.lock().unwrap_or_else(|p| p.into_inner()) =
                        value.clone().unwrap_or_else(|| "auto".to_string());
                }
                // gh #30 / ADR-0042: `permissions.mode` is grant-store
                // state as much as config state - the selector's edit
                // reaches the store through this one seam, the same way
                // `--yolo` applies it at startup. Without this the
                // footer would wear YOLO while every prompt still asked.
                if key == "permissions.mode"
                    && let Some(mode) = value
                        .as_deref()
                        .and_then(lca_permissions::PermissionMode::parse)
                {
                    grants
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .set_permission_mode(mode);
                }
                persist_ui_setting(key, value);
            });
        UiHooks {
            run_shell: Some(self.shell_runner()),
            external_editor: Some(Arc::new(external_editor)),
            persist_screen_mode: Some(Arc::new(persist_screen_mode)),
            persist_setting: Some(persist_setting),
            settings_rows: {
                let ui = self.clone();
                let data = self.data.clone();
                Some(Arc::new(move || {
                    use lca_config::{PERMISSION_MODES, SHELL_TOOLS, THINKING_VISIBILITIES};
                    use lca_ui::SettingRow;
                    // The winning layer per key (FR-CFG-2's column) -
                    // re-read now, not from the startup snapshot: the
                    // selector's own last write has to show up as the
                    // layer that won. `Chat::settings_rows` mirrors the
                    // three keys whose session value can outrun the file.
                    let config = crate::load_config_flags(
                        &ui.cwd,
                        &crate::lock(&ui.grants),
                        false,
                        false,
                        &ui.flags,
                    )
                    .unwrap_or_else(|_| ui.config.clone());
                    let resolved: std::collections::BTreeMap<
                        String,
                        (String, lca_config::MergeSource),
                    > = config
                        .resolved()
                        .map(|(key, value, source)| (key.to_string(), (value, source)))
                        .collect();
                    let row = |key: &str, values: &[&str]| {
                        let (value, source) = match key {
                            // The one setting that lives outside the
                            // config file: `ui.json` (FR-UI-21) says so
                            // in the source column rather than lying
                            // about a layer that never wrote it.
                            "ui.fullscreen" => (
                                initial_screen_mode(&data).to_string(),
                                "ui.json".to_string(),
                            ),
                            _ => resolved
                                .get(key)
                                .map(|(value, source)| (value.clone(), source.to_string()))
                                .unwrap_or_else(|| ("<unset>".to_string(), "default".to_string())),
                        };
                        SettingRow {
                            key: key.to_string(),
                            value,
                            source,
                            values: values.iter().map(|value| value.to_string()).collect(),
                        }
                    };
                    // The curated user-facing set (gh #30): raw numeric
                    // keys stay out - they are `lca config`'s business.
                    vec![
                        row("ui.theme", &[]),
                        row("ui.thinking", THINKING_VISIBILITIES),
                        row("thinking", &[]),
                        row("ui.fullscreen", &["false", "true"]),
                        row("ui.color", &["auto", "never"]),
                        row("permissions.mode", PERMISSION_MODES),
                        row("shell.tool", SHELL_TOOLS),
                    ]
                }))
            },
            cycle_model: {
                let ui = self.clone();
                Some(Arc::new(move |forward: bool| ui.cycle_model(forward)))
            },
            set_thinking: {
                let ui = self.clone();
                Some(Arc::new(move |level: Option<&str>| {
                    let model = ui
                        .model_cell
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .id
                        .clone();
                    // The clamp is the model's: a level outside its
                    // `models.thinking_levels` set becomes its default,
                    // and unset stays unset (the provider's choice).
                    let effective = ui.config.clamp_thinking(level, &model);
                    let previous = ui
                        .thinking_cell
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone();
                    *ui.thinking_cell.lock().unwrap_or_else(|p| p.into_inner()) = effective.clone();
                    let saved = crate::persist_setting("thinking", effective.as_deref());
                    let notice = match (level, effective.as_deref()) {
                        (Some(asked), Some(effort)) if asked != effort => {
                            format!("thinking: {effort} ({asked} is not offered by {model})")
                        }
                        (_, Some(effort)) => format!("thinking: {effort}"),
                        (_, None) => "thinking: provider default".to_string(),
                    };
                    // gh #47 review: the log's witness of the switch,
                    // like `model-change` - but only when the level
                    // actually moved.
                    if let Some(record) = crate::models::thinking_level_change_record(
                        previous.as_deref(),
                        effective.as_deref(),
                    ) {
                        let session = ui.session();
                        if let Err(err) = ui.store.append(&session, record) {
                            return format!("{notice} - not recorded: {err}");
                        }
                    }
                    match saved {
                        Ok(()) => notice,
                        Err(err) => format!("{notice} - not saved: {err}"),
                    }
                }))
            },
            save_default_model: Some(Arc::new(|id: &str| {
                // One key: `model` is the startup default
                // (`resolve_model_id`), the provider extension is the
                // configured one, and a model's *profile* rides its entry
                // in the models list (gh #31) - so the saved default is
                // complete without a second key. `provider` is left alone
                // deliberately: a config that holds `[provider]` as a
                // section owns its own children, and rewriting the leaf
                // would drop them.
                match crate::persist_setting("model", Some(id)) {
                    Ok(()) => format!("default model saved: {id} - new sessions start here"),
                    Err(err) => format!("could not save the default model: {err}"),
                }
            })),
            models: {
                let ui = self.clone();
                Some(Arc::new(move || {
                    super::display::model_rows(&ui.offered_models(), &ui.live_name())
                }))
            },
            trust_needed: {
                let grants = self.grants.clone();
                let cwd = self.cwd.clone();
                let registry = self.registry.clone();
                Some(Arc::new(move || {
                    // Gh #45's vote runs before the operator is asked:
                    // the first yes/no decides (remembered or not),
                    // undecided falls through to the prompt below.
                    let cwd_text = cwd.to_string_lossy().into_owned();
                    let voter = registry.clone();
                    let (vote, remember) =
                        lca_core::drive_blocking(
                            async move { voter.project_trust(&cwd_text).await },
                        );
                    match vote {
                        lca_protocol::TrustVote::Yes | lca_protocol::TrustVote::No => {
                            let trusted = vote == lca_protocol::TrustVote::Yes;
                            let mut store = grants.lock().unwrap_or_else(|p| p.into_inner());
                            if remember {
                                let _ = store.set_trusted(&cwd, trusted);
                            } else if trusted {
                                store.trust_for_session(&cwd);
                            }
                            return false;
                        }
                        lca_protocol::TrustVote::Undecided => {}
                    }
                    // Prompt only when there is something to gate: a project
                    // `.lca/config.toml` that is not trusted yet (ADR-0039).
                    let trusted = grants
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .is_trusted_here(&cwd);
                    !trusted && cwd.join(".lca").join("config.toml").is_file()
                }))
            },
            trust_apply: {
                let grants = self.grants.clone();
                let cwd = self.cwd.clone();
                Some(Arc::new(move |choice: lca_ui::state::TrustChoice| {
                    let mut store = grants.lock().unwrap_or_else(|p| p.into_inner());
                    match choice {
                        lca_ui::state::TrustChoice::Persist(trusted) => {
                            if let Err(err) = store.set_trusted(&cwd, trusted) {
                                return format!("could not save trust: {err}");
                            }
                            if trusted {
                                format!("trusted {} (remembered)", cwd.display())
                            } else {
                                format!("{} marked untrusted", cwd.display())
                            }
                        }
                        lca_ui::state::TrustChoice::Session(trusted) => {
                            if trusted {
                                store.trust_for_session(&cwd);
                                format!("trusted {} for this session", cwd.display())
                            } else {
                                format!("{} stays untrusted for this session", cwd.display())
                            }
                        }
                    }
                }))
            },
            session_tree: Some(self.session_tree()),
            session_list: Some(self.session_list()),
            switch_session: Some(self.switch_session()),
            load_attachment: Some(self.load_attachment()),
            copy_to_clipboard: Some(Arc::new(|text: &str| native_clipboard(text))),
            open_url: Some(Arc::new(open_url)),
            fork_at: Some(self.fork_at()),
            grants: Some(self.grants()),
            revoke_grant: Some(self.revoke_grant()),
            // R4: a background login/identity step reports back through the
            // loop's poll instead of blocking the input thread on a
            // 300-second OAuth callback; Escape cancels it.
            poll_login: {
                let ui = self.clone();
                Some(Arc::new(move || ui.poll_login()) as lca_ui::LoginPoll)
            },
            cancel_login: {
                let ui = self.clone();
                Some(Arc::new(move || ui.cancel_login()) as lca_ui::LoginCancel)
            },
            // `/compact` summarizes on its own thread; the loop polls the
            // state so the interface keeps painting (and the separator can
            // say `Working`) for the whole model round-trip.
            poll_compact: {
                let state = self.compact_state.clone();
                Some(Arc::new(move || {
                    let mut guard = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    // Hand off a finished compaction once, the way
                    // `poll_login` takes its result; `Running` stays put
                    // until there is something to hand over.
                    match guard.clone() {
                        lca_ui::CompactState::Done(notice) => {
                            *guard = lca_ui::CompactState::Idle;
                            lca_ui::CompactState::Done(notice)
                        }
                        ongoing => ongoing,
                    }
                }) as lca_ui::CompactPoll)
            },
            // gh #12: the transcript's pre-parse markdown pipeline rides
            // whatever native extensions provide, in registration order.
            markdown_transformers: collect_markdown_transformers(&self.registry.handles()),
        }
    }

    /// The project's grants (S8), in the store's own granularity: extension
    /// enablement and the approved proposal set are install consent (their
    /// revoke path is `ext disable` / re-consent); the ad hoc patterns are
    /// revocable in place.
    fn grants(&self) -> lca_ui::state::GrantList {
        let grants = self.grants.clone();
        let cwd = self.cwd.clone();
        Arc::new(move || {
            let store = grants.lock().unwrap_or_else(|p| p.into_inner());
            let mut out: Vec<lca_ui::state::GrantEntry> = Vec::new();
            // ADR-0039: the trust decision and the rules come first, so the
            // view explains why commands do (or do not) prompt.
            out.push(lca_ui::state::GrantEntry {
                install_consent: true,
                subject: "trust".to_string(),
                detail: if store.is_trusted_here(&cwd) {
                    "trusted".to_string()
                } else {
                    "untrusted (run /trust)".to_string()
                },
                revocable: false,
            });
            for rule in store.rules(&cwd) {
                let scope = match rule.scope {
                    lca_permissions::RuleScope::Session => "session",
                    lca_permissions::RuleScope::Project => "project",
                    lca_permissions::RuleScope::Global => "global",
                };
                let decision = match rule.decision {
                    lca_permissions::RuleDecision::Allow => "allow",
                    lca_permissions::RuleDecision::Deny => "deny",
                };
                out.push(lca_ui::state::GrantEntry {
                    install_consent: false,
                    subject: format!("rule [{scope}]"),
                    detail: format!("{decision} {}", rule.pattern),
                    revocable: false,
                });
            }
            for (name, enabled) in store.extensions(&cwd) {
                out.push(lca_ui::state::GrantEntry {
                    install_consent: true,
                    subject: name,
                    detail: if enabled {
                        "enabled".to_string()
                    } else {
                        "disabled".to_string()
                    },
                    revocable: false,
                });
            }
            for pattern in store.proposal_patterns(&cwd) {
                out.push(lca_ui::state::GrantEntry {
                    install_consent: true,
                    subject: "approved proposal".to_string(),
                    detail: pattern,
                    revocable: false,
                });
            }
            for pattern in store.patterns(&cwd) {
                out.push(lca_ui::state::GrantEntry {
                    install_consent: false,
                    subject: "ad hoc".to_string(),
                    detail: pattern,
                    revocable: true,
                });
            }
            for pattern in store.net_patterns(&cwd) {
                out.push(lca_ui::state::GrantEntry {
                    install_consent: false,
                    subject: "ad hoc net".to_string(),
                    detail: pattern,
                    revocable: true,
                });
            }
            out
        })
    }

    /// Revoke one ad hoc grant through the store's own write path (S8).
    fn revoke_grant(&self) -> lca_ui::state::GrantRevoke {
        let grants = self.grants.clone();
        let cwd = self.cwd.clone();
        Arc::new(move |entry: &lca_ui::state::GrantEntry| -> String {
            let mut store = grants.lock().unwrap_or_else(|p| p.into_inner());
            let result = if entry.subject == "ad hoc net" {
                store.revoke_net_pattern(&cwd, &entry.detail)
            } else {
                store.revoke_pattern(&cwd, &entry.detail)
            };
            match result {
                Ok(()) => format!("revoked {} - the next action will ask again", entry.detail),
                Err(err) => format!("could not revoke {}: {err}", entry.detail),
            }
        })
    }

    /// The `!`/`!!` runner (R4): the command runs on its own thread in its
    /// own process group, streamed to the card, cancellable by Escape.
    fn shell_runner(&self) -> lca_ui::ShellRunner {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        let cwd = self.cwd.clone();
        Arc::new(
            move |command: &str,
                  excluded: bool,
                  sink: std::sync::mpsc::SyncSender<ShellEvent>|
                  -> ShellHandle {
                let session = session_cell
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                // `!` is context; `!!` is not (FR-UI-14).
                if !excluded {
                    let _ = store.append(
                        &session,
                        Record::User {
                            v: lca_protocol::FORMAT_VERSION,
                            ts: lca_session::now_ms(),
                            id: lca_session::new_record_id(),
                            content: format!("!{command}"),
                            attachments: Vec::new(),
                            queue: None,
                        },
                    );
                }
                // R4: the command runs on its own thread, in its own
                // process group (`spawn_direct`), so a long command
                // never freezes the interface and Escape can kill the
                // whole tree.
                let mut child = match lca_tools::spawn_direct(
                    "sh",
                    &["-c".to_string(), command.to_string()],
                    &cwd,
                ) {
                    Ok(child) => child,
                    Err(err) => {
                        let _ = sink.send(ShellEvent::Chunk(format!("shell failed: {err}")));
                        let _ = sink.send(ShellEvent::Done(Some(1)));
                        return Arc::new(|| {});
                    }
                };
                let stdout = child.stdout();
                let stderr = child.stderr();
                let child = Arc::new(Mutex::new(child));
                let cancel_child = child.clone();
                let cancel: ShellHandle = Arc::new(move || {
                    if let Ok(mut child) = cancel_child.lock() {
                        child.kill_tree();
                    }
                });
                let out_sink = sink.clone();
                let err_sink = sink.clone();
                std::thread::spawn(move || {
                    let out = std::thread::spawn(move || read_into(stdout, &out_sink));
                    let err = std::thread::spawn(move || read_into(stderr, &err_sink));
                    // Poll rather than hold the lock across `wait()`:
                    // the cancel closure needs the lock to kill the
                    // tree, and a blocking `wait()` holding it would
                    // deadlock a long-running command (R4).
                    let code = loop {
                        let status = {
                            let mut child = child.lock().unwrap_or_else(|p| p.into_inner());
                            child.try_wait()
                        };
                        match status {
                            Ok(Some(code)) => break Some(code),
                            Ok(None) => {
                                std::thread::sleep(std::time::Duration::from_millis(50));
                            }
                            Err(_) => break None,
                        }
                    };
                    let _ = out.join();
                    let _ = err.join();
                    let _ = sink.send(ShellEvent::Done(code));
                });
                cancel
            },
        )
    }

    /// The `/tree` branch selector (FR-UI-16). Rows carry the same label
    /// the `/resume` picker does (first prompt while the session still has
    /// the default title), so a branch named `179079… * (session)` tells
    /// you which branch it is.
    fn session_tree(&self) -> lca_ui::state::SessionTree {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        let cwd = self.cwd.clone();
        Arc::new(move || {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let labels: std::collections::HashMap<String, String> = store
                .list_sessions(&cwd)
                .unwrap_or_default()
                .into_iter()
                .map(|summary| (summary.id.clone(), summary.display_title()))
                .collect();
            store
                .fork_tree(&session)
                .unwrap_or_default()
                .into_iter()
                .map(|branch| {
                    let title = labels.get(branch.id()).cloned().unwrap_or_else(|| {
                        store
                            .meta(&branch)
                            .map(|meta| meta.title)
                            .unwrap_or_default()
                    });
                    let marker = if branch.id() == session.id() {
                        " *"
                    } else {
                        ""
                    };
                    (
                        branch.id().to_string(),
                        format!("{}{marker} ({title})", branch.id()),
                    )
                })
                .collect()
        })
    }

    /// The `/resume` session list (R2), newest first with ages.
    fn session_list(&self) -> lca_ui::state::SessionList {
        let store = self.store.clone();
        let cwd = self.cwd.clone();
        Arc::new(move || {
            let now = lca_session::now_ms();
            store
                .list_sessions(&cwd)
                .unwrap_or_default()
                .into_iter()
                .map(|summary| {
                    let title = summary.display_title();
                    lca_ui::resume::SessionEntry {
                        id: summary.id,
                        title,
                        messages: summary.message_count,
                        age: age_label(now, summary.modified_ms),
                    }
                })
                .collect()
        })
    }

    /// Switch the live session in place (`/tree`, `/resume`; R3). Returns
    /// the reopened session's records - the interface replays them with the
    /// live rendering (FR-UI-7) - or `None` when the id cannot open.
    fn switch_session(&self) -> lca_ui::state::SwitchSession {
        let store = self.store.clone();
        let cwd = self.cwd.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move |id: &str| -> Option<Vec<lca_protocol::Record>> {
            let session = store.session(&cwd, id).ok()?;
            let read = store.read_with(&session, ViewMode::Display).ok()?;
            crate::init_session_temp(session.id());
            *session_cell.lock().unwrap_or_else(|p| p.into_inner()) = session;
            Some(read.records)
        })
    }

    /// Resolve an attachment hash against the *current* session (a fork's
    /// images live in its ancestor's directory, store.attachment_path walks
    /// the chain), so a replayed message shows its image (FR-UI-13).
    fn load_attachment(&self) -> lca_ui::state::LoadAttachment {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move |hash: &str| {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let path = store.attachment_path(&session, hash)?;
            let bytes = std::fs::read(path).ok()?;
            let media = lca_protocol::sniff_image_media_type(&bytes)?.to_string();
            Some((media, bytes))
        })
    }

    /// Fork at the nth user message (FR-UI-16).
    fn fork_at(&self) -> lca_ui::state::ForkAt {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move |index: usize| -> String {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let outcome = match store.read_with(&session, ViewMode::Display) {
                Ok(outcome) => outcome,
                Err(err) => return format!("cannot read the session: {err}"),
            };
            let record_id = outcome
                .records
                .iter()
                .filter(|record| matches!(record, Record::User { .. }))
                .nth(index)
                .and_then(|record| record.id().map(str::to_string));
            let Some(record_id) = record_id else {
                return format!("no user message at index {index}");
            };
            match store.fork(&session, &record_id) {
                Ok(branch) => format!(
                    "forked at message {index}: {} - resume with `lca --resume {}`",
                    branch.id(),
                    branch.id()
                ),
                Err(err) => format!("fork failed: {err}"),
            }
        })
    }
}

/// Open `$EDITOR`/`$VISUAL` on the prompt text (P6).
fn external_editor(text: &str) -> Option<String> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("EDITOR").ok().filter(|s| !s.is_empty()))?;
    external_editor_with(&editor, text, &std::env::temp_dir())
}

/// The testable core of [`external_editor`]: split the editor command on
/// shell words and spawn it directly with the buffer path as its own argv
/// element (#121). Paths with spaces need quoting nowhere: neither the
/// split nor the spawn rejoins words, and the buffer file carries a
/// unique name. An editor string that does not split (an unbalanced
/// quote) falls back to the platform shell — the one case that genuinely
/// needs one.
fn external_editor_with(editor: &str, text: &str, dir: &std::path::Path) -> Option<String> {
    let buffer = tempfile::Builder::new()
        .prefix("lca-prompt-")
        .suffix(".md")
        .tempfile_in(dir)
        .ok()?;
    // Close the handle before spawning: on Windows an open file cannot be
    // opened again by the editor. The path stays reserved (and deleted on
    // drop) through the guard below.
    let path = buffer.into_temp_path();
    std::fs::write(&path, text).ok()?;
    let status = match shell_words::split(editor) {
        Ok(words) => {
            let (program, args) = words.split_first()?;
            std::process::Command::new(program)
                .args(args)
                .arg(&path)
                .status()
                .ok()?
        }
        Err(_) => run_through_shell(&format!("{editor} {}", path.display())).ok()?,
    };
    let edited = std::fs::read_to_string(&path).ok();
    status.success().then_some(edited).flatten()
}

/// Run one command line through the platform shell: `sh -c` on Unix,
/// `cmd /C` on Windows. Only the fallback for an editor string no word
/// splitter accepts — everything else spawns directly.
fn run_through_shell(command: &str) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg(command)
            .status()
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .status()
    }
}

/// Persist a `/theme`/`/thinking` pick to the config file (E2); a write
/// failure is a warning the user can still act on, never a crash.
fn persist_ui_setting(key: &str, value: Option<String>) {
    if let Err(err) = crate::persist_setting(key, value.as_deref()) {
        tracing::warn!("could not persist {key}: {err}");
    }
}

/// Persist the screen mode across runs (FR-UI-21).
fn persist_screen_mode(fullscreen: bool) {
    let path = crate::data_dir().join("ui.json");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("{{\"fullscreen\":{fullscreen}}}"));
}

/// The screen mode a fresh session starts in (FR-UI-21).
///
/// **The default is the main-screen (terminal scrollback) renderer (S1).**
/// S1 ported pi's bottom-anchored incremental append contract so that real
/// scrollback history survives in the terminal and the terminal's native
/// selection and scrollbar just work. The alt screen is the app-owned-selection
/// opt-in (`ui.fullscreen = true` or `/fullscreen`).
///
/// A persisted `ui.json` still wins over the default, so an explicit
/// `/fullscreen` choice survives either way. ADR-0037 carries the dated
/// annotations.
pub(super) fn initial_screen_mode(config_dir: &std::path::Path) -> bool {
    std::fs::read_to_string(config_dir.join("ui.json"))
        .map(|text| text.contains("true"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: S1 (ADR-0037 third annotation) - a fresh session defaults
    // to the main-screen renderer (terminal scrollback), and a persisted
    // `/fullscreen` choice still wins either way.
    #[test]
    fn the_fresh_screen_mode_is_scrollback_and_a_persisted_pick_wins() {
        let root = lca_testkit::scratch_path("lca-screen-mode");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        // No ui.json: scrollback (main screen, false) is the default.
        assert!(!initial_screen_mode(&root));
        // A persisted fullscreen pick is honored.
        std::fs::write(root.join("ui.json"), "{\"fullscreen\":true}").expect("write");
        assert!(initial_screen_mode(&root));
        std::fs::write(root.join("ui.json"), "{\"fullscreen\":false}").expect("write");
        assert!(!initial_screen_mode(&root));
        let _ = std::fs::remove_dir_all(&root);
    }

    // The python probe mirrors scripts/traceability.sh: the repo already
    // requires a python on PATH, so the mock editor is one everywhere
    // (Windows CI included).
    fn python_on_path() -> String {
        for candidate in ["python3", "python", "py"] {
            let probing = std::process::Command::new(candidate)
                .arg("-c")
                .arg("pass")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if probing.is_ok_and(|status| status.success()) {
                return candidate.to_string();
            }
        }
        panic!("a python is required on PATH (scripts/traceability.sh needs one too)");
    }

    /// A mock `$EDITOR`: appends a marker line to the file named by its
    /// first argument. Written into `dir` (which may contain spaces).
    fn mock_editor(dir: &std::path::Path) -> (String, std::path::PathBuf) {
        let script = dir.join("mock editor.py");
        std::fs::write(
            &script,
            "import sys\npath = sys.argv[1]\ntext = open(path, encoding=\"utf-8\").read()\nopen(path, \"w\", encoding=\"utf-8\").write(text + \"[edited]\\n\")\n",
        )
        .expect("write mock editor");
        let editor = format!("{} \"{}\"", python_on_path(), script.display());
        (editor, script)
    }

    // Verifies: #121 (the editor command splits on shell words, spawns
    // directly with the buffer path as its own argv element, and round
    // trips through a directory with spaces — on every platform).
    #[test]
    fn the_editor_launches_through_spaces_directly() {
        let root = lca_testkit::scratch_path("lca-editor-spaces");
        let dir = root.join("dir with spaces");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let (editor, _) = mock_editor(&dir);
        let edited = external_editor_with(&editor, "hello", &dir).expect("editor runs");
        assert!(
            edited.contains("hello") && edited.contains("[edited]"),
            "round trip through the spaced path: {edited:?}"
        );
    }

    // Verifies: #121 (a plain `program --flag` editor splits the same way).
    #[test]
    fn the_editor_splits_flags_from_the_program() {
        let root = lca_testkit::scratch_path("lca-editor-flags");
        std::fs::create_dir_all(&root).expect("mkdir");
        let script = root.join("edit.py");
        std::fs::write(
            &script,
            "import sys\npath = sys.argv[1]\nopen(path, \"w\", encoding=\"utf-8\").write(\"flagged\\n\")\n",
        )
        .expect("write mock editor");
        let editor = format!("{} {}", python_on_path(), script.display());
        let edited = external_editor_with(&editor, "hello", &root).expect("editor runs");
        assert!(
            edited.contains("flagged"),
            "program and flag split, path appended: {edited:?}"
        );
    }
}
