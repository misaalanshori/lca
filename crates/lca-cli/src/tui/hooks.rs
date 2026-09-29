//! Host hooks (S1): the `!`/`!!` shell runner, the external editor,
//! screen-mode persistence, the session tree/list/switch/fork surfaces,
//! and the clipboard/URL openers. Each was a closure capturing `run`'s
//! locals; each is now a method on [`Ui`].

use std::sync::{Arc, Mutex};

use lca_protocol::Record;
use lca_session::ViewMode;
use lca_ui::{ShellEvent, ShellHandle, UiHooks};

use super::Ui;
use super::display::{age_label, display_line};

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

/// Open a URL in the platform's browser (R6/S7). Fire-and-forget: returns
/// `Err` with the reason when no launcher is available or every candidate
/// refused to start, so the notice can say why rather than just "cannot".
fn open_url(url: &str) -> Result<(), String> {
    let candidates: &[(&str, &[&str])] = {
        #[cfg(target_os = "macos")]
        {
            &[("open", &[] as &[&str])]
        }
        #[cfg(target_os = "windows")]
        {
            &[("cmd", &["/C", "start", ""] as &[&str])]
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            &[
                ("xdg-open", &[] as &[&str]),
                ("x-www-browser", &[] as &[&str]),
            ]
        }
    };
    let mut tried: Vec<String> = Vec::new();
    for (program, args) in candidates {
        let mut command = std::process::Command::new(program);
        command
            .args(*args)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if command.spawn().is_ok() {
            return Ok(());
        }
        tried.push((*program).to_string());
    }
    Err(format!("no URL opener found (tried {})", tried.join(", ")))
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
    pub(super) fn hooks(&self) -> UiHooks {
        // E2: keep the live theme cell in step with the persisted pick so
        // `/settings` shows the session value, like it does for thinking.
        let theme_cell = self.theme_cell.clone();
        let persist_setting: lca_ui::state::SettingPersist =
            Arc::new(move |key: &str, value: Option<String>| {
                if key == "ui.theme" {
                    *theme_cell.lock().unwrap_or_else(|p| p.into_inner()) =
                        value.clone().unwrap_or_else(|| "auto".to_string());
                }
                persist_ui_setting(key, value);
            });
        UiHooks {
            run_shell: Some(self.shell_runner()),
            external_editor: Some(Arc::new(external_editor)),
            persist_screen_mode: Some(Arc::new(persist_screen_mode)),
            persist_setting: Some(persist_setting),
            models: {
                let provider = self.provider.clone();
                Some(Arc::new(move || {
                    provider
                        .list_models()
                        .iter()
                        .map(|model| model.id.clone())
                        .collect()
                }))
            },
            session_tree: Some(self.session_tree()),
            session_list: Some(self.session_list()),
            switch_session: Some(self.switch_session()),
            copy_to_clipboard: Some(Arc::new(|text: &str| native_clipboard(text))),
            open_url: Some(Arc::new(open_url)),
            fork_at: Some(self.fork_at()),
            grants: Some(self.grants()),
            revoke_grant: Some(self.revoke_grant()),
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

    /// The `/tree` branch selector (FR-UI-16).
    fn session_tree(&self) -> lca_ui::state::SessionTree {
        let store = self.store.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move || {
            let session = session_cell
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            store
                .fork_tree(&session)
                .unwrap_or_default()
                .into_iter()
                .map(|branch| {
                    let title = store
                        .meta(&branch)
                        .map(|meta| meta.title)
                        .unwrap_or_default();
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
                .map(|summary| lca_ui::resume::SessionEntry {
                    id: summary.id,
                    title: summary.title,
                    messages: summary.message_count,
                    age: age_label(now, summary.modified_ms),
                })
                .collect()
        })
    }

    /// Switch the live session in place (`/tree`, `/resume`; R3). Returns
    /// the reopened transcript's lines, or `None` when the id cannot open.
    fn switch_session(&self) -> lca_ui::state::SwitchSession {
        let store = self.store.clone();
        let cwd = self.cwd.clone();
        let session_cell = self.current_session.clone();
        Arc::new(move |id: &str| -> Option<Vec<String>> {
            let session = store.session(&cwd, id).ok()?;
            let read = store.read_with(&session, ViewMode::Display).ok()?;
            let lines: Vec<String> = read.records.iter().filter_map(display_line).collect();
            crate::init_session_temp(session.id());
            *session_cell.lock().unwrap_or_else(|p| p.into_inner()) = session;
            Some(lines)
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
    let path = std::env::temp_dir().join(format!("lca-prompt-{}.md", std::process::id()));
    std::fs::write(&path, text).ok()?;
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} {}", path.display()))
        .status()
        .ok()?;
    let edited = std::fs::read_to_string(&path).ok();
    let _ = std::fs::remove_file(&path);
    status.success().then_some(edited).flatten()
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
    let path = crate::config_dir().join("ui.json");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("{{\"fullscreen\":{fullscreen}}}"));
}
