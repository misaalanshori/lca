//! The session subcommands: `config`, `resume`, `fork`, `rename`,
//! `export`, and `session gc` (S3's ceiling split).

use super::*;
use lca_session::Session;

/// Which session a headless run appends to (#111: `-c` continues the
/// project's most recent session, `-r <id>` resumes that session).
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SessionSelector {
    /// A fresh session.
    New,
    /// The project's most recent session.
    Continue,
    /// A named session.
    Resume(String),
    /// Fork a session at its tip and run the fork (gh #69):
    /// `--session-id` alongside chooses the fork's id.
    Fork {
        /// The parent session to fork.
        parent: String,
        /// The fork's id, when `--session-id` chose it.
        new_id: Option<String>,
    },
    /// An exact session id, created when absent (gh #69).
    Exact(String),
}

/// The headless output protocol (gh #56, pi's `--mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Today's plain output.
    Text,
    /// Today's `--json` envelope.
    Json,
    /// The stdin/stdout JSONL command loop.
    Rpc,
}

/// Resolve the output protocol: `--mode` wins, `--json` is its deprecated
/// alias (flags are stable within a major - it keeps working), and the
/// two contradicting is a usage error.
pub fn output_mode(cli: &Cli) -> Result<OutputMode, String> {
    match (cli.mode.as_deref(), cli.json) {
        (Some("rpc"), true) | (Some("text"), true) => Err(format!(
            "--json contradicts --mode {}; drop one (or use --mode json)",
            cli.mode.as_deref().unwrap_or("")
        )),
        (Some("rpc"), false) => Ok(OutputMode::Rpc),
        (Some("json"), _) | (None, true) => Ok(OutputMode::Json),
        (Some("text"), false) | (None, false) => Ok(OutputMode::Text),
        (Some(other), _) => Err(format!("unknown --mode {other:?}; use text, json, or rpc")),
    }
}

/// What `run` assembled before routing (gh #71): piped stdin already
/// read, `@file` text already expanded, `@file` images returned
/// separately for attach staging, and which streams are redirected
/// (pi's print-mode rule).
#[derive(Debug, Clone, Default)]
pub struct Invocation {
    /// Trimmed piped stdin (`None` on a terminal, when empty, or in RPC
    /// mode, which owns stdin).
    pub stdin_text: Option<String>,
    /// Expanded `@file` text, pi's `<file name>` shape (`""` when none).
    pub file_text: String,
    /// `@file` images for attach staging (headless and TUI alike).
    pub file_images: Vec<std::path::PathBuf>,
    /// Standard input is not a terminal.
    pub stdin_piped: bool,
    /// Standard output is not a terminal.
    pub stdout_piped: bool,
}

/// What a parsed command line asks for. Split out so the dispatch rule
/// itself is testable without a terminal.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Headless turns, in order, in one session (FR-CORE-3).
    Headless {
        /// The prompts to run, in order (#109: `-p` value, `--prompt`,
        /// then positionals, concatenated in that order).
        messages: Vec<String>,
        /// Model override if provided (#111).
        model: Option<String>,
        /// Which session the turns append to (#111).
        session: SessionSelector,
        /// The output protocol (gh #56).
        mode: OutputMode,
    },
    /// The interactive interface in the working directory (FR-CORE-2).
    Interactive {
        /// The session to resume, when the subcommand names one.
        resume: Option<String>,
        /// Open the session picker instead of a fresh session (gh #110:
        /// bare `-r`).
        resume_picker: bool,
        /// Model override if provided.
        model: Option<String>,
        /// Positional messages: the first is submitted on open (#109).
        initial: Vec<String>,
        /// Fork this session at its tip and open the fork (gh #69).
        fork: Option<String>,
        /// Open the exact session id, creating it when absent (gh #69).
        /// Volatile sessions (`--no-session`) ride the flags: the store
        /// root switches under the same session plumbing.
        session_id: Option<String>,
    },
    /// The merged-configuration printout (FR-CFG-2).
    Config,
    /// An extension-management subcommand (FR-DIST-*).
    Ext(ext::ExtCmd),
    /// A credential subcommand (gh #72, pi's `auth`).
    Auth(AuthCmd),
    /// The session listing (FR-SESS-2).
    ResumeList,
    /// Fork at a message (FR-SESS-3).
    Fork {
        /// Parent session id.
        session: String,
        /// Record id to fork at.
        message: String,
    },
    /// Clone a session at its tip (gh #205).
    Clone {
        /// Parent session id.
        session: String,
        /// Title for the clone (defaults to `Clone of <parent-title>`).
        title: Option<String>,
    },
    /// Rename a session.
    Rename {
        /// Session id.
        session: String,
        /// New title.
        title: String,
    },
    /// Export a session (FR-SESS-7).
    Export {
        /// Session id.
        session: String,
        /// Keep audit records.
        audit: bool,
    },
    /// Delete a session's unreferenced attachments (D5).
    Gc {
        /// Session id.
        session: String,
    },
}

/// Flag combinations that contradict each other (#109, #111). `Some`
/// carries the usage error; `run` prints it and exits 2. Kept beside
/// `route` so the rule is unit-testable without a terminal.
pub fn check_flag_contradictions(cli: &Cli) -> Option<String> {
    if cli.r#continue && cli.resume_id.is_some() {
        return Some(
            "-c/--continue and -r/--resume select different sessions; pass exactly one".to_string(),
        );
    }
    if cli.r#continue && cli.session.is_some() {
        return Some(
            "-c/--continue and --session select different sessions; pass exactly one".to_string(),
        );
    }
    if cli.resume_id.is_some() && cli.session.is_some() {
        return Some(
            "-r/--resume and --session select different sessions; pass exactly one".to_string(),
        );
    }
    // gh #69 (pi's exclusivity): `--fork` stands alone except for
    // `--session-id` (the fork's id) and `--name`; `--session-id`
    // combines with nothing but those two; `--no-session` with nothing
    // that names a session.
    if cli.fork.is_some() {
        if cli.session.is_some() {
            return Some(
                "--fork and --session select different sessions; pass exactly one".to_string(),
            );
        }
        if cli.r#continue {
            return Some(
                "--fork and -c/--continue select different sessions; pass exactly one".to_string(),
            );
        }
        if cli.resume_id.is_some() {
            return Some(
                "--fork and -r/--resume select different sessions; pass exactly one".to_string(),
            );
        }
        if cli.no_session {
            return Some(
                "--fork and --no-session contradict; fork persists the new session".to_string(),
            );
        }
    }
    if let Some(id) = cli.session_id.as_deref() {
        if !lca_session::valid_session_id(id) {
            return Some(format!(
                "--session-id `{id}` is invalid: use letters, numbers, `.`, `_`, `-`"
            ));
        }
        if cli.session.is_some() {
            return Some(
                "--session-id and --session select different sessions; pass exactly one"
                    .to_string(),
            );
        }
        if cli.r#continue {
            return Some(
                "--session-id and -c/--continue select different sessions; pass exactly one"
                    .to_string(),
            );
        }
        if cli.resume_id.is_some() {
            return Some(
                "--session-id and -r/--resume select different sessions; pass exactly one"
                    .to_string(),
            );
        }
    }
    if cli.no_session
        && (cli.session.is_some()
            || cli.r#continue
            || cli.resume_id.is_some()
            || cli.session_id.is_some())
    {
        return Some("--no-session persists nothing; drop the session selector".to_string());
    }
    // gh #69: the new session flags take no subcommand (like `--session`
    // before them); `--name` neither (subcommands name their own things).
    if cli.command.is_some()
        && (cli.fork.is_some() || cli.session_id.is_some() || cli.no_session || cli.name.is_some())
    {
        return Some(
            "a subcommand takes its own arguments; pass the session flags without one".to_string(),
        );
    }
    // Gh #110: the picker needs the interface; next to a prompt it has
    // nowhere to render, so say so instead of starting fresh silently.
    if cli.resume_id == Some(None)
        && (cli.print.is_some() || cli.prompt.is_some() || !cli.messages.is_empty())
    {
        return Some(
            "bare -r opens the session picker, which needs the interface; drop the prompt or name a session".to_string(),
        );
    }
    // Gh #110: the picker/session flags route before subcommands, so
    // naming both is refused up front instead of hijacking the command.
    if cli.command.is_some() && (cli.resume_id == Some(None) || cli.session.is_some()) {
        return Some(
            "a subcommand takes its own arguments; pass -r/--session without one".to_string(),
        );
    }
    if cli.command.is_some()
        && (cli.print.is_some() || cli.prompt.is_some() || !cli.messages.is_empty())
    {
        return Some(
            "a subcommand takes its own arguments; pass -p/--prompt and messages without one"
                .to_string(),
        );
    }
    // gh #71 (pi's shape): RPC mode owns stdin for commands, so `@file`
    // arguments have nowhere to expand.
    if cli.mode.as_deref() == Some("rpc")
        && cli.messages.iter().any(|message| message.starts_with('@'))
    {
        return Some(
            "--mode rpc takes no @file arguments; send prompt commands on stdin".to_string(),
        );
    }
    // gh #56: `--mode rpc` is a command loop, not a turn runner.
    if cli.mode.as_deref() == Some("rpc") {
        if cli.command.is_some() {
            return Some(
                "--mode rpc takes no subcommand; drive the session from stdin".to_string(),
            );
        }
        if cli.print.is_some() || cli.prompt.is_some() || !cli.messages.is_empty() {
            return Some(
                "--mode rpc takes no prompt arguments; send prompt commands on stdin".to_string(),
            );
        }
    }
    if let Err(err) = output_mode(cli) {
        return Some(err);
    }
    None
}

pub(super) fn config_command(cwd: &Path) -> i32 {
    let data = data_dir();
    let grants = match GrantStore::open(&data.join("grants.json")) {
        Ok(grants) => grants,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::INTERNAL;
        }
    };
    let config = match load_config(cwd, &grants, false, false) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    };
    // gh #30 (PG-032): print-only by design - the interactive editor is
    // the `/settings` selector, and this is the same dump `/settings`
    // used to print: every key with its winning source, the live
    // overrides labelled, `shell.resolved`, and the screen renderer.
    let shell = crate::resolve_shell(&config).ok();
    let shell_error = crate::resolve_shell(&config).err();
    print!(
        "{}",
        crate::tui::settings_text(
            &config,
            config.thinking(),
            config.ui_theme(),
            shell.as_ref(),
            shell_error.as_deref(),
        )
    );
    0
}

pub(super) fn open_store() -> Result<(SessionStore, PathBuf), i32> {
    let data = data_dir();
    Ok((SessionStore::new(data.clone()), data))
}

/// Resolve a parsed command line to a route.
pub fn route(cli: &Cli) -> Route {
    route_with(cli, &Invocation::default())
}

/// Resolve a parsed command line with its invocation (gh #71): piped
/// stdin and `@file` text prepend the first message in pi's order, and
/// a redirected stream without JSON/RPC mode means print mode.
pub fn route_with(cli: &Cli, inv: &Invocation) -> Route {
    // #109 + gh #71: every typed message becomes a positional, in one
    // stable order (the legacy `--prompt` value, then `-p`, then
    // positionals), with piped stdin and `@file` text prepended to the
    // first in pi's order. `@path` tokens split out of the positionals
    // here (a bare `-p` contributes none - see the
    // `default_missing_value` edge in `Cli`).
    let planned = crate::invoke::plan_messages(
        cli.prompt.as_deref(),
        cli.print.as_deref(),
        &cli.messages,
        inv.stdin_text.as_deref(),
        &inv.file_text,
    );
    let mut messages = Vec::new();
    messages.extend(planned.first);
    messages.extend(planned.rest);
    // Pi's redirect rule: with terminal streams the interface opens
    // unless `-p` says otherwise; a redirected stream without JSON/RPC
    // mode means print mode instead.
    let redirect = inv.stdin_piped || inv.stdout_piped;
    let structured = cli.mode.as_deref() == Some("json") || cli.mode.as_deref() == Some("rpc");
    let print_mode = cli.print.is_some() || cli.prompt.is_some() || (redirect && !structured);

    // #111: the headless session selector. A `-c`/`-r` contradiction is
    // rejected in `run` before routing (route owns no exit code). A
    // bare `-r` (the picker, gh #110) never reaches here: the
    // contradiction check refuses it next to headless flags first.
    let headless_session = if cli.r#continue {
        SessionSelector::Continue
    } else if let Some(parent) = cli.fork.clone() {
        SessionSelector::Fork {
            parent,
            new_id: cli.session_id.clone(),
        }
    } else if let Some(id) = cli.session_id.clone() {
        SessionSelector::Exact(id)
    } else if let Some(id) = cli
        .session
        .clone()
        .or_else(|| cli.resume_id.clone().flatten())
    {
        SessionSelector::Resume(id)
    } else {
        SessionSelector::New
    };

    // gh #56: `--mode rpc` is headless by itself (a command loop takes
    // no prompt arguments; the contradiction check rejects those).
    let rpc = cli.mode.as_deref() == Some("rpc");
    if (print_mode || rpc) && cli.command.is_none() {
        return Route::Headless {
            messages,
            model: cli.model.clone(),
            session: headless_session,
            mode: if rpc {
                OutputMode::Rpc
            } else if cli.json || cli.mode.as_deref() == Some("json") {
                OutputMode::Json
            } else {
                OutputMode::Text
            },
        };
    }
    // Gh #110: bare `-r` opens the picker; `--session` and `-r <id>`
    // resume direct (the reference may be a path, resolved at open).
    if cli.resume_id == Some(None) {
        return Route::Interactive {
            resume: None,
            resume_picker: true,
            model: cli.model.clone(),
            initial: messages,
            fork: cli.fork.clone(),
            session_id: cli.session_id.clone(),
        };
    }
    if let Some(id) = cli
        .session
        .clone()
        .or_else(|| cli.resume_id.clone().flatten())
    {
        return Route::Interactive {
            resume: Some(id),
            resume_picker: false,
            model: cli.model.clone(),
            initial: messages,
            fork: cli.fork.clone(),
            session_id: cli.session_id.clone(),
        };
    }
    match &cli.command {
        None => {
            if cli.r#continue {
                let data = data_dir();
                let store = SessionStore::new(data.clone());
                let cwd = std::env::current_dir().unwrap_or_default();
                let latest = store
                    .list_sessions(&cwd)
                    .ok()
                    .and_then(|list| list.into_iter().next())
                    .map(|s| s.id);
                Route::Interactive {
                    resume: latest,
                    resume_picker: false,
                    model: cli.model.clone(),
                    initial: messages,
                    fork: cli.fork.clone(),
                    session_id: cli.session_id.clone(),
                }
            } else {
                Route::Interactive {
                    resume: None,
                    resume_picker: false,
                    model: cli.model.clone(),
                    initial: messages,
                    fork: cli.fork.clone(),
                    session_id: cli.session_id.clone(),
                }
            }
        }
        Some(Command::Config) => Route::Config,
        Some(Command::Resume { id }) => match id {
            None => Route::ResumeList,
            Some(id) => Route::Interactive {
                resume: Some(id.clone()),
                resume_picker: false,
                model: cli.model.clone(),
                initial: messages,
                fork: cli.fork.clone(),
                session_id: cli.session_id.clone(),
            },
        },
        Some(Command::Fork { session, message }) => Route::Fork {
            session: session.clone(),
            message: message.clone(),
        },
        Some(Command::Clone { session, title }) => Route::Clone {
            session: session.clone(),
            title: title.clone(),
        },
        Some(Command::Rename { session, title }) => Route::Rename {
            session: session.clone(),
            title: title.clone(),
        },
        Some(Command::Export { session, audit }) => Route::Export {
            session: session.clone(),
            audit: *audit,
        },
        Some(Command::Session { cmd }) => match cmd {
            SessionCmd::Gc { session } => Route::Gc {
                session: session.clone(),
            },
        },
        Some(Command::Ext { cmd }) => Route::Ext(cmd.clone()),
        Some(Command::Auth { cmd }) => Route::Auth(cmd.clone()),
    }
}

pub(super) fn resume_list(cwd: &Path) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    match store.list_sessions(cwd) {
        Ok(sessions) => {
            if sessions.is_empty() {
                println!("No sessions yet. Start one with `lca -p \"...\"`.");
                return exit::OK;
            }
            for session in sessions {
                println!(
                    "{}\t{}\t{} messages",
                    session.id,
                    session.display_title(),
                    session.message_count
                );
            }
            exit::OK
        }
        Err(err) => {
            eprintln!("error: cannot list sessions: {err}");
            exit::SESSION
        }
    }
}

pub(super) fn fork_command(cwd: &Path, id: &str, message: &str) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    let parent = match store.session(cwd, id) {
        Ok(parent) => parent,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::SESSION;
        }
    };
    match store.fork(&parent, message) {
        Ok(child) => {
            println!("{}", child.id());
            exit::OK
        }
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

pub(super) fn clone_command(cwd: &Path, id: &str, title: Option<&str>) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    let parent = match store.session(cwd, id) {
        Ok(parent) => parent,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::SESSION;
        }
    };
    match store.clone_session(&parent, title) {
        Ok(child) => {
            println!("{}", child.id());
            exit::OK
        }
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

pub(super) fn rename_command(cwd: &Path, id: &str, title: &str) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    match store.session(cwd, id) {
        Ok(session) => match store.rename(&session, title) {
            Ok(()) => exit::OK,
            Err(err) => {
                eprintln!("error: {err}");
                exit::SESSION
            }
        },
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

pub(super) fn export_command(cwd: &Path, id: &str, audit: bool) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    match store.session(cwd, id) {
        Ok(session) => match store.export(&session, ExportOptions { audit }) {
            Ok(path) => {
                println!("{}", path.display());
                exit::OK
            }
            Err(err) => {
                eprintln!("error: {err}");
                exit::SESSION
            }
        },
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

/// `lca session gc <id>`: mark-and-sweep over the session's fork tree (D5).
/// Prints one deleted hash per line, or a line saying nothing was collected.
pub(super) fn gc_command(cwd: &Path, id: &str) -> i32 {
    let Ok((store, _)) = open_store() else {
        return exit::INTERNAL;
    };
    let session = match store.session(cwd, id) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::SESSION;
        }
    };
    match store.gc(&session) {
        Ok(deleted) => {
            if deleted.is_empty() {
                println!("no unreferenced attachments");
            } else {
                for hash in deleted {
                    println!("{hash}");
                }
            }
            exit::OK
        }
        Err(err) => {
            eprintln!("error: {err}");
            exit::SESSION
        }
    }
}

/// `lca --list-models [search]`: every model the configured provider
/// offers, as `id  provider  context` lines, then exit 0 (gh #8,
/// EFG-003's CI building block).
///
/// `provider` is the row's profile when it has one (gh #31 - the service
/// that will answer), the provider extension's name otherwise; `context`
/// is the window in tokens, `0` when the provider publishes none. The
/// output is sorted by provider then id, so a diff of two listings says
/// what changed. An optional pattern filters it through the same matcher
/// `models.enabled` uses.
pub(super) fn list_models_command(
    cwd: &Path,
    search: &str,
    cli: &CliFlags,
    allow_host: &[String],
) -> i32 {
    let data = data_dir();
    let grants = match GrantStore::open(&data.join("grants.json")) {
        Ok(grants) => std::sync::Arc::new(std::sync::Mutex::new(grants)),
        Err(err) => {
            eprintln!("error: cannot open the grant store: {err}");
            return exit::INTERNAL;
        }
    };
    let config = match load_config_flags(cwd, &lock(&grants), true, false, cli) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    };
    // The gh #31 rule holds for a listing: a discovery `GET /models` is a
    // live request to a profile endpoint, and this command cannot prompt.
    // `--allow-host` grants the host for this run (the same session set;
    // a listing has no session to record a `permission` answer in, and
    // nothing has been requested yet). An ungranted host is refused with
    // headless mode's fix rather than reached for silently.
    for host in allow_host {
        if let Err(err) = lock(&grants).attach_session_net_pattern(cwd, host) {
            eprintln!("error: {err}");
            return exit::USAGE;
        }
    }
    let provider_name = config.provider().to_string();
    // No session exists to hang a stats source on: the native set's stats
    // source answers empty, because a listing is not a turn. Assembled
    // before the consent check (gh #157) so the provider's manifest -
    // not a host literal - drives the env-var lookup.
    let stats: lca_ext_native::StatsSource = Arc::new(String::new);
    let prompt = crate::HeadlessPrompt::default();
    let shared_prompt = lca_permissions::SharedPrompt::default();
    shared_prompt.set(std::sync::Arc::new(std::sync::Mutex::new(prompt.clone())));
    // No session here (a model listing is not a turn): the system temp
    // dir, explicitly (gh #160).
    let registry = crate::registry::assemble(
        cwd,
        &config,
        shared_prompt,
        lca_permissions::SharedDialogs::default(),
        &grants,
        stats,
        &std::env::temp_dir(),
        cli,
    );
    if let Some(host) =
        crate::net_consent::env_configured_host(&data, &provider_name, Some(&registry))
        && crate::ungranted_host(&grants, cwd, Some(host.clone())).is_some()
    {
        eprintln!("{}", crate::net_consent::denied_message(&host));
        return exit::PERMISSION;
    }
    let provider: Arc<dyn lca_provider::Provider> = match registry.provider(&provider_name) {
        Some(handle) => Arc::new(lca_core::ExtensionProvider::new(handle.clone())),
        None => {
            eprintln!("{}", crate::no_model_message(&provider_name));
            return exit::USAGE;
        }
    };
    let mut rows = crate::models::filter_enabled(provider.list_models(), config.models_enabled());
    let search = search.trim();
    if !search.is_empty() {
        rows.retain(|model| crate::models::in_scope(model, &[search.to_string()]));
    }
    if prompt.needed_approval() {
        // Something the listing touched wanted a human and there is no
        // human here: say so instead of printing a list built around the
        // refusal.
        eprintln!(
            "error: listing the models needed an approval this command cannot ask for; \
             run `lca` once interactively and approve it"
        );
        return exit::PERMISSION;
    }
    if rows.is_empty() && !search.is_empty() {
        println!("no models matching \"{search}\"");
        return exit::OK;
    }
    let service = |model: &lca_protocol::ModelInfo| {
        model
            .extras
            .get("profile")
            .cloned()
            .unwrap_or_else(|| provider_name.clone())
    };
    rows.sort_by(|a, b| service(a).cmp(&service(b)).then_with(|| a.id.cmp(&b.id)));
    for model in &rows {
        println!("{}  {}  {}", model.id, service(model), model.context_window);
    }
    exit::OK
}

/// Resolve the session to open (gh #69): fork clones at the tip (with
/// `--session-id` choosing the fork's id), an exact id opens or is
/// created, otherwise the resumed one or a fresh one. `--name` titles
/// fresh sessions and renames opened ones (a rename failure warns and
/// the interface still opens).
pub fn resolve_session(
    store: &SessionStore,
    cwd: &Path,
    resume: Option<&str>,
    fork: Option<&str>,
    session_id: Option<&str>,
    name: Option<&str>,
) -> Result<lca_session::Session, String> {
    let title = name.unwrap_or(lca_session::DEFAULT_TITLE);
    if let Some(parent_id) = fork {
        let parent = store
            .session_ref(cwd, parent_id)
            .map_err(|err| format!("cannot fork session `{parent_id}`: {err}"))?;
        let parent_title = store
            .meta(&parent)
            .map(|meta| meta.title)
            .unwrap_or_default();
        let child_title = name
            .map(str::to_string)
            .unwrap_or_else(|| format!("Fork of {parent_title}"));
        let child = store
            .clone_session(&parent, Some(&child_title))
            .map_err(|err| format!("cannot fork session `{parent_id}`: {err}"))?;
        if let Some(new_id) = session_id
            && child.id() != new_id
        {
            return store
                .reid(&child, new_id)
                .map_err(|err| format!("cannot identify the fork as `{new_id}`: {err}"));
        }
        return Ok(child);
    }
    if let Some(id) = session_id {
        if let Ok(session) = store.session(cwd, id) {
            rename_quiet(store, &session, name);
            return Ok(session);
        }
        return store
            .create_session_with_id(cwd, id, title)
            .map_err(|err| format!("cannot create session `{id}`: {err}"));
    }
    match resume {
        // Gh #110: the reference may be a session-directory path.
        Some(id) => {
            let session = store.session_ref(cwd, id).map_err(|err| format!("{err}"))?;
            rename_quiet(store, &session, name);
            Ok(session)
        }
        None => store
            .create_session(cwd, title)
            .map_err(|err| format!("cannot start a session: {err}")),
    }
}

/// Apply `--name` to an opened session (gh #69): a rename failure
/// warns and the interface still opens.
fn rename_quiet(store: &SessionStore, session: &Session, name: Option<&str>) {
    if let Some(name) = name
        && let Err(err) = store.rename(session, name)
    {
        eprintln!("warning: cannot name the session `{name}`: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `Cli::parse_from` needs the derive's trait in scope; it used to
    // arrive through the crate root, which the `cli_args` split took with it.
    use clap::Parser;

    fn route_of(args: &[&str]) -> Route {
        let cli = Cli::parse_from(args);
        route(&cli)
    }

    // Verifies: issue #6 (pi's CLI surface) - `-r <id>` opens that session,
    // `resume <id>` is its long form, a bare `lca` opens fresh, `--model`
    // rides every one of them into the interface, and `-p` stays headless.
    // (#109: positionals ride `initial`; #111: headless carries the model
    // and the session selector.)
    #[test]
    fn resume_and_continue_flags_route_to_the_interface() {
        assert_eq!(
            route_of(&["lca", "-r", "abc", "--model", "m"]),
            Route::Interactive {
                resume: Some("abc".to_string()),
                resume_picker: false,
                model: Some("m".to_string()),
                initial: Vec::new(),
                fork: None,
                session_id: None,
            }
        );
        assert_eq!(
            route_of(&["lca", "resume", "abc"]),
            Route::Interactive {
                resume: Some("abc".to_string()),
                resume_picker: false,
                model: None,
                initial: Vec::new(),
                fork: None,
                session_id: None,
            }
        );
        assert_eq!(
            route_of(&["lca"]),
            Route::Interactive {
                resume: None,
                resume_picker: false,
                model: None,
                initial: Vec::new(),
                fork: None,
                session_id: None,
            }
        );
        assert_eq!(
            route_of(&["lca", "-p", "hi"]),
            Route::Headless {
                messages: vec!["hi".to_string()],
                model: None,
                session: SessionSelector::New,
                mode: OutputMode::Text,
            }
        );
    }

    // `-c` resolves this directory's latest session (whatever the machine
    // happens to hold), so the guarantee under test is the shape: it opens
    // the interface, with the model flag still attached.
    #[test]
    fn continue_opens_the_interface_with_the_model_flag() {
        match route_of(&["lca", "-c", "--model", "x"]) {
            Route::Interactive { model, .. } => {
                assert_eq!(model, Some("x".to_string()));
            }
            other => panic!("-c opens the interface, not {other:?}"),
        }
    }
}

#[cfg(test)]
mod invocation_tests {
    use super::*;
    use clap::Parser;

    fn invocation() -> Invocation {
        Invocation::default()
    }

    // Verifies: gh #71 - piped stdin and `@file` text prepend the first
    // message in pi's order; the rest ride behind.
    #[test]
    fn stdin_and_files_prepend_the_first_message() {
        let cli = Cli::parse_from(["lca", "-p", "review", "@a.txt", "second"]);
        let inv = Invocation {
            stdin_text: Some("DIFF".to_string()),
            file_text: "<file>\n".to_string(),
            ..invocation()
        };
        assert_eq!(
            route_with(&cli, &inv),
            Route::Headless {
                messages: vec!["DIFF<file>\nreview".to_string(), "second".to_string()],
                model: None,
                session: SessionSelector::New,
                mode: OutputMode::Text,
            }
        );
    }

    // Verifies: gh #71 - `--` stops option parsing, so a prompt can
    // begin with `-` (clap carries the rest as positionals).
    #[test]
    fn double_dash_stops_option_parsing() {
        let cli = Cli::parse_from(["lca", "--", "-p"]);
        assert_eq!(
            route_with(&cli, &invocation()),
            Route::Interactive {
                resume: None,
                resume_picker: false,
                model: None,
                initial: vec!["-p".to_string()],
                fork: None,
                session_id: None,
            }
        );
    }

    // Verifies: gh #71 - a redirected stream without JSON/RPC mode means
    // print mode, even with no `-p` (pi's redirect rule).
    #[test]
    fn redirected_streams_imply_print_mode() {
        let cli = Cli::parse_from(["lca", "review"]);
        let inv = Invocation {
            stdout_piped: true,
            ..invocation()
        };
        assert_eq!(
            route_with(&cli, &inv),
            Route::Headless {
                messages: vec!["review".to_string()],
                model: None,
                session: SessionSelector::New,
                mode: OutputMode::Text,
            }
        );
        // Terminal streams keep the interface.
        assert!(matches!(
            route_with(&cli, &invocation()),
            Route::Interactive { .. }
        ));
    }

    // Verifies: gh #71 - RPC mode owns stdin for commands, so `@file`
    // arguments are refused up front (pi's shape).
    #[test]
    fn rpc_mode_refuses_at_files() {
        let cli = Cli::parse_from(["lca", "--mode", "rpc", "@a.txt"]);
        assert_eq!(
            check_flag_contradictions(&cli),
            Some("--mode rpc takes no @file arguments; send prompt commands on stdin".to_string())
        );
    }
}

#[cfg(test)]
mod session_flag_tests {
    use super::*;
    use clap::Parser;

    fn check(args: &[&str]) -> Option<String> {
        let cli = Cli::parse_from(args);
        check_flag_contradictions(&cli)
    }

    // Verifies: gh #69 - `--session-id` routes exact-or-create, and a
    // malformed id refuses before touching the store.
    #[test]
    fn session_id_routes_and_validates() {
        let cli = Cli::parse_from(["lca", "--session-id", "abc-123"]);
        assert_eq!(
            route(&cli),
            Route::Interactive {
                resume: None,
                resume_picker: false,
                model: None,
                initial: Vec::new(),
                fork: None,
                session_id: Some("abc-123".to_string()),
            }
        );
        let cli = Cli::parse_from(["lca", "-p", "hi", "--session-id", "abc-123"]);
        assert_eq!(
            route(&cli),
            Route::Headless {
                messages: vec!["hi".to_string()],
                model: None,
                session: SessionSelector::Exact("abc-123".to_string()),
                mode: OutputMode::Text,
            }
        );
        assert!(check(&["lca", "--session-id", "abc-123"]).is_none());
        assert!(check(&["lca", "--session-id", "../evil"]).is_some());
        assert!(check(&["lca", "--session-id", "x-"]).is_some());
    }

    // Verifies: gh #69 - pi's exclusivity: `--fork` stands alone, and
    // `--session-id` combines with nothing but `--fork` and `--name`.
    #[test]
    fn fork_and_session_id_exclusivity() {
        assert!(check(&["lca", "--fork", "abc", "--session", "def"]).is_some());
        assert!(check(&["lca", "--fork", "abc", "-c"]).is_some());
        assert!(check(&["lca", "--fork", "abc", "-r", "def"]).is_some());
        assert!(check(&["lca", "--fork", "abc", "--no-session"]).is_some());
        assert!(check(&["lca", "--fork", "abc", "--session-id", "def"]).is_none());
        assert!(check(&["lca", "--fork", "abc", "--name", "n"]).is_none());
        assert!(check(&["lca", "--session-id", "abc", "--session", "def"]).is_some());
        assert!(check(&["lca", "--session-id", "abc", "-c"]).is_some());
        assert!(check(&["lca", "--session-id", "abc", "-r", "def"]).is_some());
        assert!(check(&["lca", "--no-session", "--session", "def"]).is_some());
        assert!(check(&["lca", "--no-session", "-p", "hi"]).is_none());
    }

    // Verifies: gh #69 - `--name` leaves routing alone (it travels
    // in the flags) but refuses subcommands, which name their own
    // things.
    #[test]
    fn name_rides_runs_and_refuses_subcommands() {
        let cli = Cli::parse_from(["lca", "-p", "hi", "--name", "demo"]);
        assert_eq!(
            route(&cli),
            Route::Headless {
                messages: vec!["hi".to_string()],
                model: None,
                session: SessionSelector::New,
                mode: OutputMode::Text,
            }
        );
        assert!(check(&["lca", "-p", "hi", "--name", "demo"]).is_none());
        // (Global flags ride before the subcommand; after it clap
        // itself refuses the unknown flag.)
        assert!(check(&["lca", "--name", "demo", "config"]).is_some());
    }
}
