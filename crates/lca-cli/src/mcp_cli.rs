//! `lca mcp add|remove|list|login|logout` (gh #418 Phase F, gh #73; pi's
//! `pi mcp`).
//!
//! A **thin** front end: every verb parses arguments, calls one
//! [`lca_mcp::cli::CliMcp`] method, and prints the result. The rules live in
//! `lca-mcp`, so the `/mcp` interface verbs and these share one code path per
//! action and cannot drift.
//!
//! These commands work outside a session, which is the point (pi's
//! `docs/cli.md`: "These commands work outside a session, so agents can run
//! them through `bash`"). They are **not** gated by `--no-mcp` or the
//! enablement record: a user debugging a disabled setup still needs `list` and
//! `login`.
//!
//! Exit codes mirror pi's `cli.ts`, which returns `1` for both usage errors
//! and operational failures and `0` for success; LCA's own `exit::USAGE` (`2`)
//! is used where clap itself refuses the command line, because that path never
//! reaches this module.

use std::path::Path;

use lca_mcp::cli::{AddSpec, CliMcp, ListReport, Scope};

use crate::cli_args::McpCmd;
use crate::exit;

/// pi's default sign-in window (`DEFAULT_LOGIN_TIMEOUT_SECONDS`).
const DEFAULT_LOGIN_TIMEOUT_SECONDS: u64 = 300;

/// How long a `list` waits for connections before reporting what it has.
///
/// pi has no bound here (its connections are promise-based and it awaits them
/// all); LCA's are on worker threads, so a hung server would otherwise make a
/// scriptable command hang. This is the same 10-second startup window the
/// first turn uses (gh #418 Phase E), and a server still connecting is
/// reported honestly rather than dropped.
const LIST_WAIT_SECONDS: u64 = 10;

/// Run one `lca mcp` verb and return the process exit code.
pub(crate) fn run(cmd: &McpCmd, cwd: &Path, trusted: bool) -> i32 {
    let data = crate::data_dir();
    // `add` and `remove` touch files only, so they need no engine and no
    // connection. Everything else may connect, so it builds the same
    // capability environment a session would.
    match cmd {
        McpCmd::Add(args) => {
            let spec = AddSpec {
                name: args.server.clone(),
                url: args.url.clone(),
                command: args.command.clone(),
                env: parse_pairs("env", &args.env),
                cwd: args.cwd.clone(),
                headers: parse_pairs("header", &args.header),
                bearer_token_env_var: args.bearer_token_env_var.clone(),
                oauth_client_id: args.oauth_client_id.clone(),
                oauth_client_secret: args.oauth_client_secret.clone(),
                oauth_callback_port: args.oauth_callback_port,
                oauth_client_name: args.oauth_client_name.clone(),
                exposure: args.exposure.clone(),
                description: args.description.clone(),
                scope: if args.local {
                    Scope::Project
                } else {
                    Scope::User
                },
            };
            let cli = open(&data, cwd, trusted);
            match cli.add(&spec) {
                Ok(outcome) => {
                    println!(
                        "{} {} MCP server {:?} in {}.",
                        if outcome.replaced {
                            "Replaced"
                        } else {
                            "Added"
                        },
                        scope_word(outcome.scope),
                        outcome.entry.name,
                        outcome.path.display()
                    );
                    // FR-PERM-9: an untrusted project's file is inert, and
                    // saying so is the difference between "it worked" and "it
                    // will work once you trust this project".
                    if outcome.scope == Scope::Project && !cli.trusted() {
                        println!(
                            "The project is not trusted, so {} is ignored until you trust it.",
                            outcome.path.display()
                        );
                    }
                    // The secret never echoes (gh #418 Phase F). Saying only
                    // that one was stored is the useful half.
                    if spec.oauth_client_secret.is_some() {
                        println!("The OAuth client secret was written to the credential store.");
                    }
                    let may_need_sign_in =
                        matches!(outcome.entry.kind, lca_mcp::config::EntryKind::Http { .. })
                            && !outcome
                                .entry
                                .http_config()
                                .is_some_and(|config| config.has_static_auth());
                    println!(
                        "Check it with: lca mcp list{}",
                        if may_need_sign_in {
                            format!(
                                ". If it requires sign-in: lca mcp login {}",
                                outcome.entry.name
                            )
                        } else {
                            String::new()
                        }
                    );
                    exit::OK
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    exit::INTERNAL
                }
            }
        }
        McpCmd::Remove { server, local } => {
            let scope = if *local { Scope::Project } else { Scope::User };
            let cli = CliMcp::open_files_only(&data, cwd, trusted);
            match cli.remove(server, scope) {
                Ok(outcome) if outcome.removed => {
                    println!(
                        "Removed {} MCP server {:?} from {}.",
                        scope_word(scope),
                        server,
                        outcome.path.display()
                    );
                    exit::OK
                }
                Ok(outcome) => {
                    // Nothing was removed. pi's shape: name the scope and the
                    // file, then add the other-scope hint when the entry lives
                    // there. A remove that removed nothing is a refusal, never
                    // a success (gh #418 Phase F fix-up).
                    let hint = match outcome.elsewhere {
                        Some(Scope::Project) => " It is defined in project; use --local.",
                        Some(Scope::User) => " It is defined in user; omit --local.",
                        None => "",
                    };
                    eprintln!(
                        "error: no {} MCP server named {:?} in {}.{hint} See what exists with: lca mcp list",
                        scope_word(scope),
                        server,
                        outcome.path.display()
                    );
                    exit::INTERNAL
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    exit::INTERNAL
                }
            }
        }
        McpCmd::List { json } => {
            let cli = open(&data, cwd, trusted);
            let report = match cli.list(std::time::Duration::from_secs(LIST_WAIT_SECONDS)) {
                Ok(report) => report,
                Err(err) => {
                    eprintln!("error: {err}");
                    return exit::INTERNAL;
                }
            };
            let errors = cli.config_errors(&data);
            let report = ListReport { errors, ..report };
            if *json {
                match serde_json::to_string_pretty(&report) {
                    Ok(text) => println!("{text}"),
                    Err(err) => {
                        eprintln!("error: cannot encode the report: {err}");
                        return exit::INTERNAL;
                    }
                }
            } else {
                print_list(&report);
                if !cli.trusted() && cli.project_config().is_file() {
                    println!(
                        "{} is ignored because the project is not trusted.",
                        cli.project_config().display()
                    );
                }
            }
            if report.is_failure() {
                exit::INTERNAL
            } else {
                exit::OK
            }
        }
        McpCmd::Login { server, timeout } => {
            let seconds = timeout.unwrap_or(DEFAULT_LOGIN_TIMEOUT_SECONDS);
            if seconds == 0 {
                eprintln!("error: --timeout must be a positive number of seconds");
                return exit::INTERNAL;
            }
            let cli = open(&data, cwd, trusted);
            match cli.login(server, std::time::Duration::from_secs(seconds)) {
                Ok(outcome) => {
                    if let Some(url) = &outcome.auth_url {
                        println!("Sign in to MCP server {server:?} in your browser:\n{url}");
                    }
                    println!(
                        "Signed in to MCP server {server:?} ({} tool{}).",
                        outcome.tools,
                        if outcome.tools == 1 { "" } else { "s" }
                    );
                    exit::OK
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    exit::INTERNAL
                }
            }
        }
        McpCmd::Logout { server } => {
            // Needs the engine (credentials live on it) but nothing connected.
            let cli = open_credentials_only(&data, cwd, trusted);
            match cli.logout(server) {
                Ok(outcome) => {
                    if outcome.had {
                        println!("Signed out of MCP server {server:?}.");
                    } else {
                        println!("No stored credentials for MCP server {server:?}.");
                    }
                    exit::OK
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    exit::INTERNAL
                }
            }
        }
    }
}

/// Build the engine without a manager: the credential verbs (`logout`) need
/// the store but never a connection, so they spawn nothing.
fn open_credentials_only(data: &Path, cwd: &Path, trusted: bool) -> CliMcp {
    let prompt = std::sync::Arc::new(std::sync::Mutex::new(CommandPrompt));
    let caps = engine(data, cwd, trusted, prompt);
    CliMcp::open_engine_only(caps, data, cwd, trusted)
}

/// Build the engine and manager one command-scoped invocation uses.
///
/// The grants are the manager's own (`process`, workspace `fs`, one ad hoc
/// `net` pattern per configured host, `oauth`, `credentials`), so a `list` or
/// `login` can reach exactly what a session would and nothing more. The prompt
/// denies: these verbs run unattended, and a spawn that asked would hang a
/// script (the denial is recorded, and `--yolo` is not a thing here).
fn open(data: &Path, cwd: &Path, trusted: bool) -> CliMcp {
    open_with(
        data,
        cwd,
        trusted,
        std::sync::Arc::new(std::sync::Mutex::new(CommandPrompt)),
    )
}

/// A prompt for an unattended management verb.
///
/// The user typed `lca mcp list` — that is the consent, and it covers exactly
/// this invocation: `Once` is per-call, so a second spawn in the same command
/// asks again and nothing is persisted. The permission layer therefore stays
/// on the path (the deny path is what a non-interactive run gets without this),
/// and a script cannot silently widen anything: it can run the command, which
/// is the thing it was already allowed to run.
///
/// This is deliberately *not* `Always`: `list` must not teach the grant store
/// to auto-approve a server's spawn in later sessions.
struct CommandPrompt;

impl lca_permissions::PermissionPrompt for CommandPrompt {
    fn ask(&mut self, _action: &lca_permissions::Action) -> lca_permissions::Decision {
        lca_permissions::Decision::Once
    }

    fn review_proposals(&mut self, _diff: &lca_permissions::ProposalDiff) -> bool {
        false
    }
}

/// [`open`] with an explicit prompt (the tests' seam).
fn open_with(
    data: &Path,
    cwd: &Path,
    trusted: bool,
    prompt: std::sync::Arc<std::sync::Mutex<dyn lca_permissions::PermissionPrompt>>,
) -> CliMcp {
    CliMcp::open(engine(data, cwd, trusted, prompt), data, cwd, trusted)
}

/// Build the capability engine a management verb runs on.
///
/// The grants are the manager's own (`process`, workspace `fs`, one ad hoc
/// `net` pattern per configured host, `oauth`, `credentials`), so a `list` or
/// `login` can reach exactly what a session would and nothing more.
fn engine(
    data: &Path,
    cwd: &Path,
    trusted: bool,
    prompt: std::sync::Arc<std::sync::Mutex<dyn lca_permissions::PermissionPrompt>>,
) -> std::sync::Arc<lca_tools::Capabilities> {
    let loaded = lca_mcp::config::load_entries(data, cwd, trusted);
    let (grants, _warnings) = lca_mcp::manager::manager_grants(&loaded.entries);
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        lca_permissions::GrantStore::open(&data.join("grants.json"))
            .unwrap_or_else(|_| lca_permissions::GrantStore::empty()),
    ));
    let shared = lca_permissions::SharedPrompt::default();
    shared.set(prompt);
    crate::extension_capabilities(
        cwd,
        "mcp",
        grants,
        shared,
        store,
        lca_tools::ResourceSource::None,
        &data.join("tmp"),
    )
}

/// `user` / `project`, as the copy spells them.
fn scope_word(scope: Scope) -> &'static str {
    match scope {
        Scope::User => "user",
        Scope::Project => "project",
    }
}

/// Parse repeatable `KEY=VALUE` options, dropping malformed ones.
///
/// The parser already accepted them as strings; a pair with no `=` cannot mean
/// anything, and pi errors on it. This keeps the CLI thin, so the caller's
/// validation is the one that reports.
fn parse_pairs(option: &str, pairs: &[String]) -> Vec<(String, String)> {
    pairs
        .iter()
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            if key.is_empty() {
                eprintln!("error: --{option} expects KEY=VALUE, got {pair:?}");
                None
            } else {
                Some((key.to_string(), value.to_string()))
            }
        })
        .collect()
}

/// Print the human-readable report (pi's layout: one line per server, then
/// the indented details).
fn print_list(report: &ListReport) {
    if report.servers.is_empty() && report.errors.is_empty() {
        println!("No MCP servers configured. Add one with: lca mcp add <server> -- <command>");
    }
    for server in &report.servers {
        let state = match server.state.as_str() {
            "connected" => format!(
                "connected, {} tool{}",
                server.tools.len(),
                if server.tools.len() == 1 { "" } else { "s" }
            ),
            "needs-sign-in" => "needs sign-in".to_string(),
            other => other.to_string(),
        };
        println!(
            "{}: {} ({}, {})",
            server.name, state, server.exposure, server.scope
        );
        println!("  {}", server.transport);
        if server.state == "needs-sign-in" {
            println!("  sign in with: lca mcp login {}", server.name);
        }
        if !server.tools.is_empty() {
            let tools: Vec<String> = server
                .tools
                .iter()
                .map(|tool| match server.tool_exposure.get(tool) {
                    Some(exposure) => format!("{tool} [{exposure}]"),
                    None => tool.clone(),
                })
                .collect();
            println!("  tools: {}", tools.join(", "));
        }
        if server.resources {
            println!("  offers resources");
        }
        if let Some(error) = &server.error {
            for line in error.lines() {
                println!("  {line}");
            }
        }
    }
    for error in &report.errors {
        println!("config error: {error}");
    }
}
