//! Host hooks (S1): the `!`/`!!` shell runner, the external editor,
//! screen-mode persistence, the session tree/list/switch/fork surfaces,
//! and the clipboard/URL openers. Each was a closure capturing `run`'s
//! locals; each is now a method on [`Ui`].

use std::sync::{Arc, Mutex};

use lca_protocol::Record;
use lca_ui::{ShellEvent, ShellHandle, UiHooks};

/// Whether project-local extensions wait on trust (gh #138): the
/// directory exists and holds at least one entry, so there is
/// something for the trust prompt to gate. Empty or absent costs a
/// stat, never a prompt.
///
/// gh #463: from `$HOME`, `cwd/.lca/extensions` **is** `~/.lca/extensions`,
/// where installed extensions live. Counting those as "waiting" offered a
/// "Trust this project folder?" prompt for the user's own data home - the same
/// root confusion as the false `skipping project extension` warning, reached
/// through a second path. A directory that is the data home has nothing
/// project-local in it, so it does not wait.
fn project_extensions_wait(cwd: &std::path::Path, data_dir: &std::path::Path) -> bool {
    let dir = cwd.join(".lca").join("extensions");
    if crate::project_ext::same_path(&dir, &data_dir.join("extensions")) {
        return false;
    }
    std::fs::read_dir(dir)
        .map(|entries| entries.take(1).any(|entry| entry.is_ok()))
        .unwrap_or(false)
}

use super::Ui;
use super::clipboard::native_clipboard;
use super::collect_markdown_transformers;
use super::editor::external_editor;

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

/// The levels `/thinking` offers (gh #41, gh #242): the user's
/// `[models.thinking_levels]` set wins; else the model's advertised
/// `thinking_levels` extras filter the canonical order (unknown names
/// drop); else everything, the historical behavior for unknowing
/// models. `Some("")` (advertised empty) means the model does not
/// reason: the empty offer makes the picker notice instead.
fn offered_levels(configured: Option<&[String]>, advertised: Option<&str>) -> Vec<String> {
    const ORDER: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];
    if let Some(allowed) = configured {
        return ORDER
            .iter()
            .filter(|level| allowed.iter().any(|name| name == *level))
            .map(|level| (*level).to_string())
            .collect();
    }
    match advertised {
        Some(list) => {
            let advertised: Vec<&str> = list.split(',').map(str::trim).collect();
            ORDER
                .iter()
                .filter(|level| advertised.contains(level))
                .map(|level| (*level).to_string())
                .collect()
        }
        None => ORDER.iter().map(|level| (*level).to_string()).collect(),
    }
}

impl Ui {
    /// The host hooks wired for this composition (S1).
    pub(super) fn hooks(self: &Arc<Self>) -> UiHooks {
        fn current_manager(
            cell: &Arc<Mutex<Arc<lca_mcp::manager::McpManager>>>,
        ) -> Arc<lca_mcp::manager::McpManager> {
            crate::lock(cell).clone()
        }
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
            // Gh #407: an interactive `!` command inherits the terminal, the
            // same way `$EDITOR` does. The loop has already stopped the
            // interface (and its reader) before this runs, so the child gets the
            // real TTY rather than a pipe.
            run_interactive: {
                let cwd = self.cwd.clone();
                Some(Arc::new(move |command: &str| {
                    crate::tui::editor::interactive_run(&cwd, command)
                }) as lca_ui::state::InteractiveShell)
            },
            persist_screen_mode: Some(Arc::new(persist_screen_mode)),
            persist_setting: Some(persist_setting),
            // Gh #397: the project layer's own writer, and the trust bit that
            // says whether a write there can take effect. Both are honest about
            // failure: a picker that cannot write says so instead of showing a
            // state it did not reach.
            persist_project_setting: {
                let cwd = self.cwd.clone();
                Some(Arc::new(move |key: &str, value: Option<String>| {
                    crate::persist_project_setting(&cwd, key, value.as_deref())
                        .map_err(|err| err.to_string())
                }) as lca_ui::ProjectSettingPersist)
            },
            project_layer_trusted: {
                let grants = self.grants.clone();
                let cwd = self.cwd.clone();
                Some(Arc::new(move || crate::lock(&grants).is_trusted_here(&cwd))
                    as Arc<dyn Fn() -> bool + Send + Sync>)
            },
            settings_rows: super::display::settings_rows(self, self.data.clone()),
            persona_list: self.persona_list_hook(),
            persona_check: self.persona_check_hook(),
            persona_apply: self.persona_apply_hook(),
            cycle_model: {
                let ui = self.clone();
                Some(Arc::new(move |forward: bool| ui.cycle_model(forward)))
            },
            thinking_offered: {
                let ui = self.clone();
                // gh #41: the picker offers the current model's allowed
                // set in canonical order (a model without `high` never
                // shows it); models without a set offer everything.
                // Gh #242: the user's `[models.thinking_levels]` set
                // wins; else the model's advertised `thinking_levels`
                // extras filter the order (unknown names drop, empty
                // means no reasoning and the picker notices instead).
                Some(Arc::new(move || {
                    let model = ui
                        .model_cell
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .id
                        .clone();
                    let config = crate::lock(&ui.config);
                    let advertised = ui
                        .all_models()
                        .into_iter()
                        .find(|info| info.id == model)
                        .and_then(|info| {
                            info.extras
                                .get(lca_protocol::THINKING_LEVELS_EXTRA)
                                .cloned()
                        });
                    offered_levels(
                        config.allowed_thinking_levels(&model),
                        advertised.as_deref(),
                    )
                }))
            },
            // Gh #42/#402: the loop asks after every turn; the helper reports
            // only a cause it has not reported yet, so quiet turns stay quiet.
            // The seen-cause set lives inside the hook (see `display`).
            cache_miss_notice: Some(Self::cache_miss_notice_hook(self)),
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
                    let effective = crate::lock(&ui.config).clamp_thinking(level, &model);
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
            // Gh #412: `/persona`. Three seams, and the split is the atomic
            // guarantee:
            //
            // - `persona_list` reads the config's bundles that apply here (an
            //   untrusted project's never loaded, so they cannot be listed).
            // - `persona_check` answers "is every field reachable **in this
            //   session**" - the model in the offered set, the level offered by
            //   *that* model, every tool registered. It writes nothing.
            // - `persona_apply` runs only after a clean check, and writes through
            //   the same paths `/model`, `/thinking` and the tool gate use.
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
            prompt_templates: {
                let cwd = self.cwd.clone();
                Some(Arc::new(move || {
                    lca_tools::prompts::collect(&crate::prompt_roots(&cwd))
                }))
            },
            double_escape_action: {
                let ui = self.clone();
                // Reads the live cell: a `/settings` cycle applies to
                // the next Esc Esc without a restart.
                Some(Arc::new(move || {
                    crate::lock(&ui.config)
                        .ui_double_escape_action()
                        .to_string()
                }))
            },
            display_tuning: {
                let ui = self.clone();
                // Reads the live cell every frame: a `/settings`
                // cycle applies without a restart.
                Some(Arc::new(move || {
                    let config = crate::lock(&ui.config);
                    lca_ui::state::DisplayTuning {
                        autocomplete_max_visible: config.ui_autocomplete_max_visible().clamp(3, 20)
                            as u8,
                        editor_padding_x: config.ui_editor_padding_x().min(3) as u8,
                        output_pad: config.ui_output_pad().min(1) as u8,
                        fullscreen_scrollbar: config.ui_fullscreen_scrollbar().to_string(),
                        fullscreen_copy_on_select: config.ui_fullscreen_copy_on_select(),
                        fullscreen_wheel_lines: config.ui_fullscreen_wheel_lines().to_string(),
                        fullscreen_exit_output: config.ui_fullscreen_exit_output().to_string(),
                        show_images: config.terminal_show_images(),
                        image_width_cells: config.terminal_image_width_cells().max(1) as u16,
                        show_progress: config.terminal_show_progress(),
                        auto_resize_images: config.images_auto_resize(),
                        code_block_indent: config.markdown_code_block_indent().to_string(),
                        mermaid: config.markdown_mermaid().to_string(),
                    }
                }))
            },
            current_session_id: {
                let ui = self.clone();
                Some(Arc::new(move || {
                    crate::lock(&ui.current_session).id().to_string()
                }))
            },
            scoped_models: {
                let ui = self.clone();
                Some(Arc::new(move || {
                    let live = ui.live_name();
                    let scope = crate::lock(&ui.config).models_enabled().to_vec();
                    ui.all_models()
                        .iter()
                        .filter_map(|model| {
                            let (id, label) = super::display::model_label(model, &live)?;
                            Some(lca_ui::state::ScopedModelRow {
                                id,
                                label,
                                context: model.context_window,
                                enabled: crate::models::in_scope(model, &scope),
                            })
                        })
                        .collect()
                }))
            },
            save_scoped_models: {
                let ui = self.clone();
                Some(Arc::new(move |ids: Vec<String>| {
                    // pi's normalize rule: a set covering every offered
                    // model is no restriction - persist the empty scope.
                    let all: Vec<String> =
                        ui.all_models().into_iter().map(|model| model.id).collect();
                    let ids = if !all.is_empty() && all.iter().all(|id| ids.contains(id)) {
                        Vec::new()
                    } else {
                        ids
                    };
                    if let Err(err) = crate::persist_setting_list("models.enabled", &ids) {
                        return format!("could not persist models.enabled: {err}");
                    }
                    super::apply_models_scope(&ui.config, ids.clone());
                    if ids.is_empty() {
                        "rotation covers every model (models.enabled cleared)".to_string()
                    } else {
                        format!(
                            "rotation: {} model{} in scope",
                            ids.len(),
                            if ids.len() == 1 { "" } else { "s" }
                        )
                    }
                }))
            },
            trust_needed: {
                let grants = self.grants.clone();
                let cwd = self.cwd.clone();
                let registry = self.registry().clone();
                Some(Arc::new(move || {
                    // gh #80: a startup override settles the question
                    // before any vote or prompt: forced distrust and a
                    // session refusal both mean no ask.
                    {
                        let store = grants.lock().unwrap_or_else(|p| p.into_inner());
                        if store.is_force_untrusted() || store.is_refused_for_session(&cwd) {
                            return false;
                        }
                    }
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
                    // `.lca/config.toml` that is not trusted yet (ADR-0039),
                    // or project-local extensions waiting on that same
                    // trust (gh #138).
                    let trusted = grants
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .is_trusted_here(&cwd);
                    // gh #463: the same `cwd == data_dir` check the extension
                    // scan makes, because a project config file is also the
                    // user's own when the project root is the data home.
                    let data_dir = crate::data_dir();
                    let project_config_is_the_user_file =
                        crate::project_ext::same_path(&cwd.join(".lca"), &data_dir);
                    !trusted
                        && !project_config_is_the_user_file
                        && (cwd.join(".lca").join("config.toml").is_file()
                            || project_extensions_wait(&cwd, &data_dir))
                }))
            },
            stage_bug: {
                // Gh #411 gaps 2 and 3: stage a **scrubbed copy**, show the user
                // what it contains, and write only if they confirm. The closure
                // owns the whole plan, so the interface holds a string and a
                // decision and never sees a secret or a zip.
                let cwd = self.cwd.clone();
                let session = self.session().id().to_string();
                Some(Arc::new(move || {
                    let data = crate::data_dir();
                    let plan = crate::bug::stage(&data, &cwd, Some(&session), &cwd)
                        .map_err(|err| format!("bug failed: {err}"))?;
                    let inspection = plan.inspection();
                    let destination = plan.destination().display().to_string();
                    let plan = std::sync::Arc::new(plan);
                    let resolve: lca_ui::state::BugResolve = {
                        let plan = plan.clone();
                        Arc::new(move |confirmed: bool| {
                            if !confirmed {
                                crate::bug::discard(&plan);
                                return "bug bundle discarded; nothing was written".to_string();
                            }
                            match crate::bug::write(&plan) {
                                Ok(written) => format!(
                                    "wrote {} ({}); share it explicitly - nothing was uploaded",
                                    written.path.display(),
                                    written.scrubbed.summary()
                                ),
                                Err(err) => format!("bug failed: {err}"),
                            }
                        })
                    };
                    Ok(lca_ui::state::BugReview {
                        inspection,
                        destination,
                        resolve,
                    })
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
                                store.set_force_untrusted(false);
                            }
                            if trusted {
                                format!("trusted {} (remembered)", cwd.display())
                            } else {
                                format!("{} marked untrusted", cwd.display())
                            }
                        }
                        lca_ui::state::TrustChoice::Session(trusted) => {
                            if trusted {
                                store.set_force_untrusted(false);
                                store.trust_for_session(&cwd);
                                format!("trusted {} for this session", cwd.display())
                            } else {
                                format!("{} stays untrusted for this session", cwd.display())
                            }
                        }
                    }
                }))
            },
            session_list: Some(self.session_list()),
            switch_session: Some(self.switch_session()),
            load_attachment: Some(self.load_attachment()),
            copy_to_clipboard: Some(Arc::new(|text: &str| native_clipboard(text))),
            open_url: Some(Arc::new(open_url)),
            session_tree: Some(self.entry_tree()),
            new_session: Some(self.new_session()),
            tree_filter_mode: Some(self.tree_filter_mode()),
            label_record: Some(self.label_record()),
            branch_here: Some(self.branch_here()),
            rename_session: Some(self.rename_session()),
            fork_at: Some(self.fork_at()),
            fork_record: Some(self.fork_record()),
            set_label: Some(self.set_label()),
            list_labels: Some(self.list_labels()),
            delete_session: Some(self.delete_session()),
            clone_session: Some(self.clone_session()),
            reload: Some(self.reload_hook()),
            grants: Some(self.grants()),
            revoke_grant: Some(self.revoke_grant()),
            // gh #53: the MCP manager behind the verb hooks. The cell
            // swaps on `/reload`; the bridge rebuilds in place.
            mcp_action: {
                let mcp = self.mcp.clone();
                Some(Arc::new(move |verb: &str, target: &str| {
                    current_manager(&mcp).act(verb, target)
                }) as lca_ui::McpAction)
            },
            poll_mcp: {
                let mcp = self.mcp.clone();
                Some(Arc::new(move || current_manager(&mcp).poll()) as lca_ui::McpPoll)
            },
            // Gh #418 Phase G: the `/mcp` modal reads data rows and notices,
            // never widgets - `lca-mcp` composes no display (ADR-0047), so the
            // conversion to the UI's own types happens here, in the host.
            mcp_list: {
                let mcp = self.mcp.clone();
                let warnings = self.mcp_warnings.clone();
                Some(Arc::new(move || {
                    let snapshot = current_manager(&mcp).modal_rows(&warnings);
                    lca_ui::McpSnapshot {
                        rows: snapshot.rows.into_iter().map(to_ui_row).collect(),
                        warnings: snapshot.warnings,
                    }
                }) as lca_ui::McpList)
            },
            mcp_detail: {
                let mcp = self.mcp.clone();
                Some(Arc::new(move |server: &str| {
                    current_manager(&mcp).modal_detail(server).map(to_ui_detail)
                }) as lca_ui::McpDetail)
            },
            mcp_begin_sign_in: {
                let mcp = self.mcp.clone();
                Some(Arc::new(move |server: &str| {
                    current_manager(&mcp).begin_interactive_sign_in(server)
                }) as lca_ui::McpBeginSignIn)
            },
            mcp_submit_redirect: {
                let mcp = self.mcp.clone();
                Some(Arc::new(move |server: &str, redirect: &str| {
                    current_manager(&mcp).submit_redirect(server, redirect)
                }) as lca_ui::McpSubmitRedirect)
            },
            mcp_cancel_sign_in: {
                let mcp = self.mcp.clone();
                Some(Arc::new(move |server: &str| {
                    current_manager(&mcp).cancel_sign_in(server);
                }) as lca_ui::McpCancelSignIn)
            },
            // gh #3: the `/ext` manager reads local state (registry +
            // grant store + config), so every hook is synchronous.
            // Built beside the row assembly; assigned here.
            list_managed: {
                let (list, _, _) = self.extman_hooks();
                Some(list)
            },
            toggle_managed: {
                let (_, toggle, _) = self.extman_hooks();
                Some(toggle)
            },
            set_managed_strategy: {
                let (_, _, strategy) = self.extman_hooks();
                Some(strategy)
            },
            // #227: the hub fetches rows on the chat-spawned worker
            // and installs through consent; both polls below keep the
            // interface painting while each runs.
            fetch_extensions: {
                let (fetch, _, _) = self.hub_hooks();
                Some(fetch)
            },
            confirm_install: {
                let (_, confirm, _) = self.hub_hooks();
                Some(confirm)
            },
            poll_install: {
                let (_, _, poll) = self.hub_hooks();
                Some(poll)
            },
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
            markdown_transformers: collect_markdown_transformers(&self.registry().handles()),
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
                            parent: None,
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

    /// Re-run discovery without restarting (gh #130): settings files,
    /// extensions, prompts (via the rebuilt agent config), plus the
    /// themes and keybindings the report carries for the interface.
    /// Turns hold their own registry clone, so the swap never disturbs
    /// a running turn - but the command itself is refused mid-turn, so
    /// a reload cannot land between a turn's declare and its dispatch.
    /// What does NOT reload: the provider/model generation (use `/model`
    /// and `/login`), credentials, the shell backend, markdown
    /// transformers, and interface chrome already applied (colors,
    /// visibility) - those follow after restart.
    /// The `/reload` hook: a fresh discovery over the live session.
    fn reload_hook(self: &std::sync::Arc<Self>) -> lca_ui::state::Reload {
        let ui = self.clone();
        std::sync::Arc::new(move || ui.reload())
    }

    fn reload(&self) -> lca_ui::state::ReloadReport {
        use lca_ui::state::ReloadReport;
        let fail = |reason: String| ReloadReport {
            notice: format!("reload failed: {reason}"),
            themes: Vec::new(),
            key_bindings: Default::default(),
            keybinding_error: None,
        };
        let config = match crate::load_config_flags(
            &self.cwd,
            &crate::lock(&self.grants),
            false,
            self.yolo,
            &self.flags,
        ) {
            Ok(config) => config,
            Err(err) => return fail(err.to_string()),
        };
        let trusted = crate::lock(&self.grants).is_trusted(&self.cwd);
        let (mut fresh, mcp_manager, mcp_warnings) = super::startup::load_registry(
            &self.cwd,
            &config,
            self.shared_prompt.clone(),
            self.shared_dialogs.clone(),
            &self.grants,
            &self.store,
            &self.current_session,
            &self.temp_dir,
            &self.flags,
        );
        // Gh #53: `/reload` re-reads `mcp.json` too (files stay
        // hand-editable); its warnings join the report.
        *crate::lock(&self.mcp) = mcp_manager;
        let live = crate::lock(&self.live);
        let model_id = crate::lock(&self.model_cell).id.clone();
        let window = super::model_context_window(live.provider.as_ref(), &model_id);
        let image_policy = crate::models::image_policy_for(&live.provider.list_models(), &model_id);
        let session_id = crate::lock(&self.current_session).id().to_string();
        let backend = crate::registry::register_compaction(
            &mut fresh,
            &live.provider,
            &model_id,
            &session_id,
            &self.cwd,
            self.shared_prompt.clone(),
            &self.grants,
            &self.temp_dir,
        );
        let fresh = std::sync::Arc::new(fresh);
        let agent_config = match super::agent_config_for(
            &config,
            &live.name,
            &model_id,
            window,
            image_policy,
            &fresh,
            backend.map(|backend| backend as std::sync::Arc<dyn lca_tools::CompletionBackend>),
            &self.cwd,
            &self.flags,
            trusted,
        ) {
            Ok(agent_config) => agent_config,
            Err(err) => return fail(err),
        };
        let extensions = fresh.registered_names().len();
        drop(live);
        let mut live = crate::lock(&self.live);
        live.agent_config = agent_config;
        *crate::lock(&self.registry) = fresh;
        // Gh #204: the re-read settings take effect live too, so a
        // hand-edited `models.enabled` applies without a restart.
        *crate::lock(&self.config) = config;
        let (bindings, bindings_error) = crate::load_user_keybindings(&self.data);
        let mut notice =
            format!("reloaded: {extensions} extensions, settings, prompts, themes, keybindings");
        for warning in mcp_warnings {
            notice.push_str("\nwarning: ");
            notice.push_str(&warning);
        }
        ReloadReport {
            notice,
            themes: lca_ui::theme::theme_names_all(&crate::invoke::theme_extra_dirs(
                &self.flags,
                &self.cwd,
                &self.data,
            )),
            key_bindings: bindings,
            keybinding_error: bindings_error,
        }
    }
}

/// Persist a `/theme`/`/thinking` pick to the config file (E2); a write
/// failure is a warning the user can still act on, never a crash.
pub(super) fn persist_ui_setting(key: &str, value: Option<String>) {
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
/// Precedence (gh #112): a persisted `ui.json` wins when present, so an
/// explicit `/fullscreen` choice survives either way; else the config
/// file's `ui.fullscreen`; else scrollback. ADR-0037 carries the dated
/// annotations.
///
/// What `ui.json` says, if anything: typed, so `{"fullscreen": false,
/// "other": true}` stays scrollback instead of matching a substring.
pub(super) enum UiJson {
    /// No file: nothing persisted.
    Absent,
    /// The persisted choice.
    Fullscreen(bool),
    /// Unparseable or non-boolean: the message names the problem.
    Malformed(String),
}

pub(super) fn read_ui_json(config_dir: &std::path::Path) -> UiJson {
    let text = match std::fs::read_to_string(config_dir.join("ui.json")) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return UiJson::Absent,
        Err(err) => return UiJson::Malformed(format!("cannot read ui.json: {err}")),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(err) => return UiJson::Malformed(format!("ui.json is not JSON: {err}")),
    };
    match value.get("fullscreen").and_then(|flag| flag.as_bool()) {
        Some(fullscreen) => UiJson::Fullscreen(fullscreen),
        None => UiJson::Malformed("ui.json has no boolean \"fullscreen\"".to_string()),
    }
}

/// The startup renderer plus an optional warning for the transcript
/// head: `(fullscreen, warning)`. Malformed persistence falls back to
/// the config file (then scrollback) and says so loudly.
pub(super) fn initial_screen_mode(
    config_dir: &std::path::Path,
    config_fullscreen: Option<bool>,
) -> (bool, Option<String>) {
    match read_ui_json(config_dir) {
        UiJson::Absent => (config_fullscreen.unwrap_or(false), None),
        UiJson::Fullscreen(fullscreen) => (fullscreen, None),
        UiJson::Malformed(problem) => (
            config_fullscreen.unwrap_or(false),
            Some(format!("warning: {problem}; using the config default")),
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
        assert_eq!(initial_screen_mode(&root, None), (false, None));
        // A persisted fullscreen pick is honored.
        std::fs::write(root.join("ui.json"), "{\"fullscreen\":true}").expect("write");
        assert_eq!(initial_screen_mode(&root, None), (true, None));
        std::fs::write(root.join("ui.json"), "{\"fullscreen\":false}").expect("write");
        assert_eq!(initial_screen_mode(&root, None), (false, None));
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: gh #112 - an unrelated `true` in ui.json does not
    // enable fullscreen (the old substring parse did); the config
    // file fills in only when nothing persisted.
    #[test]
    fn an_unrelated_true_does_not_enable_fullscreen() {
        let root = lca_testkit::scratch_path("lca-screen-mode-112");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            root.join("ui.json"),
            "{\"fullscreen\": false, \"other\": true}",
        )
        .expect("write");
        assert_eq!(initial_screen_mode(&root, Some(true)), (false, None));
        std::fs::remove_file(root.join("ui.json")).expect("remove");
        assert_eq!(initial_screen_mode(&root, Some(true)), (true, None));
        assert_eq!(initial_screen_mode(&root, Some(false)), (false, None));
        assert_eq!(initial_screen_mode(&root, None), (false, None));
        let _ = std::fs::remove_dir_all(&root);
    }

    // Verifies: gh #112 - malformed persistence falls back and says so
    // loudly instead of substring-matching its way fullscreen.
    #[test]
    fn malformed_persistence_falls_back_with_a_warning() {
        let root = lca_testkit::scratch_path("lca-screen-mode-bad");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        for body in ["not json", "{\"fullscreen\": \"yes\"}", "{}"] {
            std::fs::write(root.join("ui.json"), body).expect("write");
            let (mode, warning) = initial_screen_mode(&root, None);
            assert!(!mode, "scrollback fallback: {body}");
            assert!(
                warning.as_deref().unwrap_or_default().contains("ui.json"),
                "loud: {body}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // The python probe mirrors scripts/traceability.sh: the repo already
    // requires a python on PATH, so the mock editor is one everywhere
    // (Windows CI included).
    pub(crate) fn python_on_path_for_tests() -> String {
        python_on_path()
    }

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
}

// Verifies: gh #242 - the offered set resolves user config first,
// then the model's advertised extras, then everything: an empty
// advertisement means no reasoning (the picker notices instead).
#[test]
fn offered_levels_prefer_config_then_extras_then_all() {
    assert_eq!(
        offered_levels(Some(&["low".to_string()]), Some("low,medium,high")),
        vec!["low".to_string()],
        "config wins over extras"
    );
    assert_eq!(
        offered_levels(None, Some("high,low")),
        vec!["low".to_string(), "high".to_string()],
        "extras filter to canonical order"
    );
    assert_eq!(
        offered_levels(None, Some("bogus")),
        Vec::<String>::new(),
        "unknown names drop"
    );
    assert_eq!(
        offered_levels(None, Some("")),
        Vec::<String>::new(),
        "advertised empty means no reasoning"
    );
    assert_eq!(offered_levels(None, None).len(), 7, "unknown offers all");
}

/// One manager row as the modal's own type (gh #418 Phase G).
///
/// The conversion lives in the host, not in `lca-mcp` and not in `lca-ui`: the
/// subsystem answers with data (ADR-0047 forbids it rendering), and the interface
/// cannot name the subsystem's types (the architecture test forbids that edge).
/// The host is the only crate that may know both.
fn to_ui_row(row: lca_mcp::ModalServerRow) -> lca_ui::McpServerRow {
    lca_ui::McpServerRow {
        scope: row.scope.to_string(),
        name: row.name,
        state: row.state,
        detail: row.detail,
        effective_exposure: row.effective_exposure,
        project_override: row.project_override,
    }
}

/// One server's detail as the modal's own type.
fn to_ui_detail(detail: lca_mcp::ModalServerDetail) -> lca_ui::McpServerDetail {
    lca_ui::McpServerDetail {
        tools: detail
            .tools
            .into_iter()
            .map(|tool| lca_ui::McpToolRow {
                name: tool.name,
                exposure: tool.exposure,
                description: tool.description,
            })
            .collect(),
        name: detail.name,
        state: detail.state,
        detail: detail.detail,
        target: detail.target,
        source_path: detail.source_path,
        override_path: detail.override_path,
        effective_exposure: detail.effective_exposure,
        configured_exposure: detail.configured_exposure,
        enabled: detail.enabled,
        project_override: detail.project_override,
        project_toggle_available: detail.project_toggle_available,
        needs_sign_in: detail.needs_sign_in,
        has_oauth: detail.has_oauth,
        stderr_tail: detail.stderr_tail,
    }
}
