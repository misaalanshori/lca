//! The shell-facing MCP verbs (gh #418 Phase F, gh #73; pi's `pi mcp`).
//!
//! One code path per action: the `/mcp` interface verbs in
//! [`crate::manager`] and these CLI verbs both drive the same
//! [`McpManager`], so a rule fixed in one place is fixed in both. What differs
//! is only presentation — the interface answers with a notice string, the CLI
//! answers with a report and an exit code.
//!
//! These verbs are **not** gated by `--no-mcp` or the enablement record: a
//! user debugging a disabled setup needs `list` and `login` to work. `list`'s
//! connections are command-scoped, so they live for exactly one invocation.

use std::path::{Path, PathBuf};

use crate::config::{
    EntryKind, McpExposure, ServerEntry, ServerSource, file_defines, load_entries,
    project_config_path, remove_entry, user_config_path, write_full_entry,
};
use crate::manager::McpManager;

/// One server's row, as `lca mcp list` reports it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServerReport {
    /// pi's server name.
    pub name: String,
    /// `user`, `project`, or `inline` (which file defined it).
    pub scope: String,
    /// How the model reaches its tools.
    pub exposure: String,
    /// Whether it is switched on.
    pub enabled: bool,
    /// The connection state: `connected`, `disabled`, `connecting`,
    /// `needs-sign-in`, or `failed`.
    pub state: String,
    /// The transport in one line (`stdio: <command>` or `http: <url>`).
    pub transport: String,
    /// The tool names the server lists (empty unless connected).
    pub tools: Vec<String>,
    /// Per-tool exposure overrides that differ from the server default.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub tool_exposure: std::collections::BTreeMap<String, String>,
    /// Whether the server offers resources.
    pub resources: bool,
    /// Why it is not connected, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ServerReport {
    /// Whether this row should make `list` exit non-zero: an enabled server
    /// that is not connected is a failure the caller can act on.
    pub fn is_failure(&self) -> bool {
        self.enabled && self.state != "connected"
    }
}

/// Everything `lca mcp list` reports: the rows plus the load warnings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ListReport {
    /// One row per configured server, in name order.
    pub servers: Vec<ServerReport>,
    /// Config-level problems (invalid entries are reported and skipped).
    pub errors: Vec<String>,
}

impl ListReport {
    /// Whether anything in the report is a failure (pi's rule: a config error
    /// or a non-connected enabled server).
    pub fn is_failure(&self) -> bool {
        !self.errors.is_empty() || self.servers.iter().any(ServerReport::is_failure)
    }
}

/// Which `mcp.json` a verb writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Scope {
    /// `<data>/mcp.json`.
    #[default]
    User,
    /// `<cwd>/.lca/mcp.json`.
    Project,
}

/// What `lca mcp add` supplies, already parsed and validated by the caller's
/// argument parser.
#[derive(Debug, Clone, Default)]
pub struct AddSpec {
    /// The server's name.
    pub name: String,
    /// A streamable-HTTP endpoint; mutually exclusive with `command`.
    pub url: Option<String>,
    /// A stdio command and its arguments.
    pub command: Vec<String>,
    /// `KEY=VALUE` pairs for a stdio child's environment.
    pub env: Vec<(String, String)>,
    /// A stdio child's working directory.
    pub cwd: Option<String>,
    /// HTTP headers.
    pub headers: Vec<(String, String)>,
    /// `Authorization: Bearer ${NAME}` for an HTTP server.
    pub bearer_token_env_var: Option<String>,
    /// A pre-registered OAuth client id.
    pub oauth_client_id: Option<String>,
    /// Its secret (kept in the credential namespace, never echoed).
    pub oauth_client_secret: Option<String>,
    /// A fixed OAuth callback port.
    pub oauth_callback_port: Option<u16>,
    /// The client name sent at registration.
    pub oauth_client_name: Option<String>,
    /// How the model reaches its tools.
    pub exposure: Option<String>,
    /// What the server offers.
    pub description: Option<String>,
    /// Where to write.
    pub scope: Scope,
}

/// The outcome of one `add`: what was written, and whether it replaced.
#[derive(Debug, Clone)]
pub struct AddOutcome {
    /// The file written.
    pub path: PathBuf,
    /// Which scope that file is.
    pub scope: Scope,
    /// Whether an entry of that name already lived there.
    pub replaced: bool,
    /// The written entry (for the caller's copy).
    pub entry: ServerEntry,
}

/// The outcome of one `remove`.
#[derive(Debug, Clone)]
pub struct RemoveOutcome {
    /// Which scope the caller asked for.
    pub scope: Scope,
    /// The file that was checked.
    pub path: PathBuf,
    /// Whether this call actually removed an entry.
    ///
    /// gh #418 Phase F fix-up: the CLI used to read `elsewhere.is_none()` as
    /// "removed", so a server present in *neither* scope returned
    /// `elsewhere: None` and the command claimed `Removed …` with exit 0. The
    /// distinction has to be a field, not an inference from another field's
    /// absence.
    pub removed: bool,
    /// `Some(scope)` when the server exists in the *other* scope, so the
    /// caller can print the `--local` hint the spec requires.
    pub elsewhere: Option<Scope>,
}

/// One loaded manager plus where its files are.
///
/// Built by [`CliMcp::open`], which is the single entry point every verb uses;
/// the CLI never assembles its own file paths or grants, so the `/mcp`
/// interface and the shell cannot drift.
///
/// `manager` is `None` for the file-only verbs (`add`, `remove`): connecting a
/// server to delete its config line would spawn a process for no reason, and
/// the drop would then have to kill it.
pub struct CliMcp {
    /// The engine. Present whenever the caller supplied one; the credential
    /// verbs need it (the store lives on an engine) even though they connect
    /// nothing.
    caps: Option<std::sync::Arc<lca_tools::Capabilities>>,
    manager: Option<McpManager>,
    /// The entries as loaded from disk. Kept even for the file-only verbs, so
    /// `logout` can resolve a name and clear its credentials without
    /// connecting anything (gh #418 Phase F: connecting to delete a token is
    /// the same mistake as connecting to delete a config line).
    entries: Vec<ServerEntry>,
    user_path: PathBuf,
    project_path: PathBuf,
    cwd: PathBuf,
    trusted: bool,
}

impl CliMcp {
    /// Load the configured servers for one command invocation.
    ///
    /// `trusted` decides whether the project's `.lca/mcp.json` participates
    /// (FR-PERM-9: an untrusted project's file is inert). Connections start on
    /// background workers exactly as they do for a session; a verb that needs
    /// them settled says so.
    pub fn open(
        caps: std::sync::Arc<lca_tools::Capabilities>,
        data_dir: &Path,
        cwd: &Path,
        trusted: bool,
    ) -> CliMcp {
        let user_path = user_config_path(data_dir);
        let project_path = project_config_path(cwd);
        let loaded = load_entries(data_dir, cwd, trusted);
        let manager = McpManager::load(
            caps.clone(),
            user_path.clone(),
            trusted.then(|| project_path.clone()),
            loaded.entries.clone(),
        );
        CliMcp {
            caps: Some(caps),
            manager: Some(manager),
            entries: loaded.entries,
            user_path,
            project_path,
            cwd: cwd.to_path_buf(),
            trusted,
        }
    }

    /// Open for a verb that needs **no** engine and **no** connections:
    /// `add` and `remove` only edit files.
    pub fn open_files_only(data_dir: &Path, cwd: &Path, trusted: bool) -> CliMcp {
        Self::open_bare(None, data_dir, cwd, trusted)
    }

    /// Open for a verb that needs the **engine but no connections**: the
    /// credential verbs (`logout`), whose store lives on an engine while
    /// nothing about them starts a server.
    pub fn open_engine_only(
        caps: std::sync::Arc<lca_tools::Capabilities>,
        data_dir: &Path,
        cwd: &Path,
        trusted: bool,
    ) -> CliMcp {
        Self::open_bare(Some(caps), data_dir, cwd, trusted)
    }

    fn open_bare(
        caps: Option<std::sync::Arc<lca_tools::Capabilities>>,
        data_dir: &Path,
        cwd: &Path,
        trusted: bool,
    ) -> CliMcp {
        let user_path = user_config_path(data_dir);
        let project_path = project_config_path(cwd);
        let loaded = load_entries(data_dir, cwd, trusted);
        CliMcp {
            caps,
            manager: None,
            entries: loaded.entries,
            user_path,
            project_path,
            cwd: cwd.to_path_buf(),
            trusted,
        }
    }

    /// The manager, for the verbs that need connections.
    fn manager(&self) -> Result<&McpManager, String> {
        self.manager
            .as_ref()
            .ok_or_else(|| "this command does not connect to any server".to_string())
    }

    /// The manager (the tests' seam).
    pub fn manager_opt(&self) -> Option<&McpManager> {
        self.manager.as_ref()
    }

    /// The path a verb writes for `scope`.
    fn path_for(&self, scope: Scope) -> &Path {
        match scope {
            Scope::User => &self.user_path,
            Scope::Project => &self.project_path,
        }
    }

    /// Where the project's `mcp.json` lives (for the untrusted note).
    pub fn project_config(&self) -> &Path {
        &self.project_path
    }

    /// Whether the project is trusted (an untrusted project's file is inert).
    pub fn trusted(&self) -> bool {
        self.trusted
    }

    /// Add or replace one server (gh #418 Phase F, pi's `add`).
    ///
    /// File-only: no server is contacted and none is spawned, which is why
    /// [`CliMcp::open_files_only`] is the right constructor for it.
    ///
    /// Refuses a spec that names both transports or neither, and refuses an
    /// option that only applies to the other transport — pi's rule, and the
    /// reason the error text names the transport rather than the flag.
    pub fn add(&self, spec: &AddSpec) -> Result<AddOutcome, String> {
        let entry = entry_from_spec(spec)?;
        let path = self.path_for(spec.scope).to_path_buf();
        let replaced = file_defines(&path, &spec.name);
        write_full_entry(&path, &entry)?;
        // The secret is never written to `mcp.json` (a config file gets
        // copied, pasted, and committed): it goes to the extension's
        // credential namespace, keyed the same way the tokens are.
        if let (Some(secret), Some(config)) =
            (spec.oauth_client_secret.as_deref(), entry.http_config())
        {
            crate::TokenStore::new(self.caps()?, &config).set_client_secret(secret)?;
        }
        Ok(AddOutcome {
            path,
            scope: spec.scope,
            replaced,
            entry,
        })
    }

    /// Remove one server, reporting the other scope when that is where it
    /// lives (pi: "use --local" / "omit --local", exit 1).
    pub fn remove(&self, name: &str, scope: Scope) -> Result<RemoveOutcome, String> {
        let path = self.path_for(scope).to_path_buf();
        if remove_entry(&path, name)? {
            // A user-level entry may also be overridden in the project file;
            // removing the defining entry leaves the override inert (it
            // references nothing), so it is cleaned up the same way.
            let other = match scope {
                Scope::User => Some(Scope::Project),
                Scope::Project => Some(Scope::User),
            };
            if let Some(other) = other {
                let other_path = self.path_for(other);
                if file_defines(other_path, name) && other == Scope::Project {
                    // Only the project *override* is dropped here; removing
                    // the other scope's real entry is the caller's decision.
                    let _ = remove_entry(other_path, name);
                }
            }
            return Ok(RemoveOutcome {
                scope,
                path,
                removed: true,
                elsewhere: None,
            });
        }
        let other = match scope {
            Scope::User => Scope::Project,
            Scope::Project => Scope::User,
        };
        let elsewhere = file_defines(self.path_for(other), name).then_some(other);
        Ok(RemoveOutcome {
            scope,
            path,
            removed: false,
            elsewhere,
        })
    }

    /// Connect every enabled server (in parallel), report, and settle.
    ///
    /// Each server already connects on its own worker (gh #418 Phase E), so
    /// "in parallel" is structural here rather than something this function
    /// arranges.
    pub fn list(&self, budget: std::time::Duration) -> Result<ListReport, String> {
        let manager = self.manager()?;
        manager.wait_until_settled(budget);
        let statuses = manager.bridge().statuses();
        let entries = manager.entries();
        let servers = statuses
            .iter()
            .map(|status| {
                let entry = entries.iter().find(|entry| entry.name == status.name);
                let (transport, resources) = entry
                    .map(transport_of)
                    .unwrap_or_else(|| (String::new(), false));
                let tool_exposure = entry
                    .map(|entry| {
                        entry
                            .tool_exposure
                            .iter()
                            .filter(|(_, exposure)| *exposure != entry.exposure)
                            .map(|(tool, exposure)| {
                                (tool.clone(), exposure_name(*exposure).to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let (state, error, tools) = match &status.state {
                    crate::ServerStateKind::Connected => {
                        let tools = manager
                            .bridge()
                            .tool_specs()
                            .map(|specs| {
                                specs
                                    .into_iter()
                                    .filter(|spec| {
                                        spec.namespace.as_ref().is_some_and(|namespace| {
                                            namespace.name == crate::mcp_namespace(&status.name)
                                        })
                                    })
                                    .map(|spec| spec.name)
                                    .collect()
                            })
                            .unwrap_or_default();
                        ("connected".to_string(), None, tools)
                    }
                    crate::ServerStateKind::Disabled => ("disabled".to_string(), None, Vec::new()),
                    crate::ServerStateKind::Connecting => {
                        ("connecting".to_string(), None, Vec::new())
                    }
                    crate::ServerStateKind::NeedsSignIn(detail) => (
                        "needs-sign-in".to_string(),
                        Some(detail.clone()),
                        Vec::new(),
                    ),
                    crate::ServerStateKind::Failed(detail) => {
                        ("failed".to_string(), Some(detail.clone()), Vec::new())
                    }
                };
                ServerReport {
                    name: status.name.clone(),
                    scope: match status.source {
                        ServerSource::User => "user".to_string(),
                        ServerSource::Project => "project".to_string(),
                        ServerSource::Inline => "inline".to_string(),
                    },
                    exposure: exposure_name(status.exposure).to_string(),
                    enabled: status.enabled,
                    state,
                    transport,
                    tools,
                    tool_exposure,
                    resources,
                    error,
                }
            })
            .collect();
        Ok(ListReport {
            servers,
            errors: Vec::new(),
        })
    }

    /// Load warnings (invalid entries are reported and skipped, never fatal).
    pub fn config_errors(&self, data_dir: &Path) -> Vec<String> {
        load_entries(data_dir, &self.cwd, self.trusted).warnings
    }

    /// Sign in to one server, blocking until the flow finishes.
    ///
    /// The browser and the loopback callback are the whole interaction here;
    /// the redirect-URL paste fallback lives in the flow itself so a headless
    /// or SSH run can still complete (FR-PROV-4's manual delivery seam).
    pub fn login(&self, name: &str, budget: std::time::Duration) -> Result<LoginOutcome, String> {
        let manager = self.manager()?;
        let entry = manager
            .entries()
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| {
                let configured: Vec<String> = manager
                    .entries()
                    .into_iter()
                    .map(|entry| entry.name)
                    .collect();
                format!(
                    "no MCP server named {name:?}. Configured: {}",
                    if configured.is_empty() {
                        "none".to_string()
                    } else {
                        configured.join(", ")
                    }
                )
            })?;
        let Some(config) = entry.http_config() else {
            return Err(format!(
                "MCP server {name:?} is a stdio server, which has no sign-in"
            ));
        };
        if config.has_static_auth() {
            return Err(format!(
                "MCP server {name:?} authenticates with a header, so there is nothing to sign in to"
            ));
        }
        if config.oauth.is_none() {
            return Err(format!("MCP server {name:?} configures no OAuth"));
        }
        // Capture the challenge first, so RFC 9728 discovery has the hint a
        // 401 supplies (gh #418 Phase D's threading).
        let _ = crate::remote::remember_unauthorized(&self.caps()?, &config, &config.url);
        // `--timeout` is the caller's budget for the whole flow; without
        // this the flag was accepted and ignored, and the wait used the
        // grant's 300-second default (gh #418 Phase F).
        let caps = self.caps()?;
        caps.oauth_set_timeout(budget.as_secs().max(1));
        crate::sign_in(caps, &config)?;
        let auth_url = self.caps()?.oauth_opened().last().cloned();
        if auth_url.is_none() {
            return Err("the sign-in published no authorization URL".to_string());
        }
        manager.drain_landed();
        let tools = self.tool_count(name);
        Ok(LoginOutcome { auth_url, tools })
    }

    /// Delete one server's stored OAuth credentials.
    pub fn logout(&self, name: &str) -> Result<LogoutOutcome, String> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("no MCP server named {name:?}"))?;
        let Some(config) = entry.http_config() else {
            return Err(format!(
                "MCP server {name:?} is a stdio server, which stores no credentials"
            ));
        };
        let store = crate::TokenStore::new(self.caps()?, &config);
        let had = store.load()?.is_some();
        store.clear()?;
        Ok(LogoutOutcome { had })
    }

    /// The capabilities the manager's engine runs with (the sign-in paths need
    /// the same engine the session uses, so a flow host grant lands where the
    /// session would have put it).
    fn caps(&self) -> Result<std::sync::Arc<lca_tools::Capabilities>, String> {
        self.caps
            .clone()
            .ok_or_else(|| "this command needs no capability engine".to_string())
    }

    /// How many tools one server currently lists.
    fn tool_count(&self, name: &str) -> usize {
        let Ok(manager) = self.manager() else {
            return 0;
        };
        manager
            .bridge()
            .tool_specs()
            .map(|specs| {
                specs
                    .iter()
                    .filter(|spec| {
                        spec.namespace
                            .as_ref()
                            .is_some_and(|namespace| namespace.name == crate::mcp_namespace(name))
                    })
                    .count()
            })
            .unwrap_or(0)
    }
}

/// The outcome of one `login`.
#[derive(Debug, Clone)]
pub struct LoginOutcome {
    /// The authorization URL the flow published (for a headless caller that
    /// wants to print it).
    pub auth_url: Option<String>,
    /// How many tools the server lists afterwards.
    pub tools: usize,
}

/// The outcome of one `logout`.
#[derive(Debug, Clone)]
pub struct LogoutOutcome {
    /// Whether credentials were actually stored.
    pub had: bool,
}

/// pi's exposure word for one value.
fn exposure_name(exposure: McpExposure) -> &'static str {
    match exposure {
        McpExposure::Direct => "direct",
        McpExposure::Codemode => "codemode",
        McpExposure::Deferred => "deferred",
        McpExposure::Hidden => "hidden",
    }
}

/// The one-line transport description `list` prints.
fn transport_of(entry: &ServerEntry) -> (String, bool) {
    match &entry.kind {
        EntryKind::Stdio { command, args, .. } => (
            format!("stdio: {command} {}", args.join(" "))
                .trim_end()
                .to_string(),
            false,
        ),
        EntryKind::Http { url, .. } => (format!("http: {url}"), false),
    }
}

/// Build the entry one `add` spec describes.
///
/// Validation lives here, not in the CLI parser, so the same rules apply to
/// any future caller (and the parser stays a thin shape).
fn entry_from_spec(spec: &AddSpec) -> Result<ServerEntry, String> {
    if !crate::is_valid_server_name(&spec.name) {
        return Err(format!(
            "invalid MCP server name {:?}: letters, digits, `_`, `-` only",
            spec.name
        ));
    }
    let http = spec.url.is_some();
    let stdio = !spec.command.is_empty();
    if http == stdio {
        return Err(
            "give either --url <url> (streamable HTTP) or `-- <command> [args...]` (stdio), not both and not neither"
                .to_string(),
        );
    }
    // pi's rule: an option that only applies to the other transport is a
    // usage error, not a silently ignored flag.
    if http {
        if !spec.env.is_empty() {
            return Err("--env only applies to stdio servers (omit --url)".to_string());
        }
        if spec.cwd.is_some() {
            return Err("--cwd only applies to stdio servers (omit --url)".to_string());
        }
    } else {
        for (flag, present) in [
            ("--header", !spec.headers.is_empty()),
            (
                "--bearer-token-env-var",
                spec.bearer_token_env_var.is_some(),
            ),
            ("--oauth-client-id", spec.oauth_client_id.is_some()),
            ("--oauth-client-secret", spec.oauth_client_secret.is_some()),
            ("--oauth-callback-port", spec.oauth_callback_port.is_some()),
            ("--oauth-client-name", spec.oauth_client_name.is_some()),
        ] {
            if present {
                return Err(format!("{flag} only applies to HTTP servers (--url)"));
            }
        }
    }
    let exposure = match &spec.exposure {
        None => McpExposure::default(),
        Some(word) => McpExposure::parse(word).map_err(|err| err.to_string())?,
    };
    let kind = if let Some(url) = &spec.url {
        let mut headers: std::collections::BTreeMap<String, String> =
            spec.headers.iter().cloned().collect();
        if let Some(name) = &spec.bearer_token_env_var {
            headers.insert("Authorization".to_string(), format!("Bearer ${{{name}}}"));
        }
        let oauth = if spec.oauth_client_id.is_some()
            || spec.oauth_client_secret.is_some()
            || spec.oauth_callback_port.is_some()
            || spec.oauth_client_name.is_some()
        {
            Some(crate::OAuthConfig {
                client_name: spec.oauth_client_name.clone(),
                scope: None,
                auth_server_metadata_url: None,
                client_id: spec.oauth_client_id.clone(),
                client_secret: spec.oauth_client_secret.clone(),
            })
        } else {
            None
        };
        EntryKind::Http {
            url: url.clone(),
            headers,
            timeout_secs: 0,
            oauth,
        }
    } else {
        let (command, args) = spec
            .command
            .split_first()
            .ok_or_else(|| "the stdio command is empty".to_string())?;
        EntryKind::Stdio {
            command: command.clone(),
            args: args.to_vec(),
            cwd: spec.cwd.clone(),
            env: spec.env.iter().cloned().collect(),
        }
    };
    Ok(ServerEntry {
        name: spec.name.clone(),
        kind,
        enabled: true,
        exposure,
        tool_exposure: Vec::new(),
        description: spec.description.clone().unwrap_or_default(),
        source: match spec.scope {
            Scope::User => ServerSource::User,
            Scope::Project => ServerSource::Project,
        },
    })
}
