//! MCP management (gh #53 phase 3): `mcp.json` loading with the
//! house trust rules, the session manager, and the system-prompt
//! section. The bridge stays in `extensions/mcp`; this module is
//! the host side (files, grants, notices).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The user's file: `<data>/mcp.json` (pi's `~/.pi/agent/mcp.json`
/// shape, LCA's data dir).
pub fn user_config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("mcp.json")
}

/// The project's file: `<cwd>/.lca/mcp.json`, read only when trusted
/// (the config file's own rule, FR-PERM-9).
pub fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".lca").join("mcp.json")
}

/// What one load found.
pub struct LoadedMcp {
    /// The merged entries, sorted by name.
    pub entries: Vec<mcp::config::ServerEntry>,
    /// One line per skipped entry or unreadable file.
    pub warnings: Vec<String>,
}

/// Load user entries plus the trusted project's (an untrusted
/// project's file is ignored silently, like the config file's).
pub fn load_entries(data_dir: &Path, cwd: &Path, trusted: bool) -> LoadedMcp {
    let mut warnings = Vec::new();
    let user_path = user_config_path(data_dir);
    let mut parsed = match std::fs::read_to_string(&user_path) {
        Ok(text) => mcp::config::parse_mcp_json(&text, mcp::config::ServerSource::User),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(err) => {
            warnings.push(format!("mcp.json unreadable: {err}"));
            Default::default()
        }
    };
    warnings.extend(std::mem::take(&mut parsed.warnings));
    let project_path = project_config_path(cwd);
    if trusted && project_path.is_file() {
        match std::fs::read_to_string(&project_path) {
            Ok(text) => {
                mcp::config::apply_project_json(&mut parsed, &text);
                warnings.extend(std::mem::take(&mut parsed.warnings));
            }
            Err(err) => warnings.push(format!("project mcp.json unreadable: {err}")),
        }
    }
    LoadedMcp {
        entries: parsed.servers,
        warnings,
    }
}

/// The host half of one URL: `scheme://host[:port]` becomes the ad
/// hoc pattern (configuration is the consent, FR-PERM-16's
/// exact-host rule).
fn url_host(url: &str) -> Option<String> {
    let after = url.split("://").nth(1)?;
    let host = after.split('/').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// The grants the manager's engine runs with: `process` for stdio
/// spawns, workspace `fs` for their scope, one ad hoc pattern per
/// configured remote host (naming the exact host the user wrote),
/// `oauth` for the sign-in loop, and `credentials` for the token
/// store. Unparsable hosts warn and connect-deny later, never
/// silently.
pub fn manager_grants(
    entries: &[mcp::config::ServerEntry],
) -> (lca_tools::CapabilityGrants, Vec<String>) {
    let mut warnings = Vec::new();
    let mut grants = mcp::manifest_grants();
    for entry in entries {
        let mcp::config::EntryKind::Http { url, .. } = &entry.kind else {
            continue;
        };
        let Some(host) = url_host(url) else {
            warnings.push(format!("server {:?} has no usable host", entry.name));
            continue;
        };
        match lca_permissions::parse_net_pattern(&host) {
            Ok(pattern) => {
                if !grants.adhoc_net.contains(&pattern) {
                    grants.adhoc_net.push(pattern);
                }
            }
            Err(err) => warnings.push(format!(
                "server {:?} grants nothing ({}); its calls will deny",
                entry.name, err
            )),
        }
    }
    grants.oauth = Some(lca_permissions::OAuthSettings {
        redirect_path: "/callback".to_string(),
        timeout_seconds: 300,
    });
    grants.credentials = true;
    (grants, warnings)
}

/// One background sign-in: the flow runs on a thread, the result
/// arrives through [`McpManager::poll`].
struct LoginTask {
    server: String,
    done: std::sync::mpsc::Receiver<(String, Result<(), String>)>,
}

/// The session manager: entries plus files, one bridge, background
/// sign-ins. Rebuilds (not reconnects-one) after every edit: with a
/// handful of servers the uniform path beats per-server surgery,
/// and the registry keeps one handle throughout.
pub struct McpManager {
    caps: Arc<lca_tools::Capabilities>,
    user_path: PathBuf,
    project_path: Option<PathBuf>,
    entries: Mutex<Vec<mcp::config::ServerEntry>>,
    bridge: Arc<mcp::McpBridge>,
    login: Mutex<Option<LoginTask>>,
}

impl McpManager {
    /// Load, grant, and connect (all through `assemble`).
    pub fn load(
        caps: Arc<lca_tools::Capabilities>,
        user_path: PathBuf,
        project_path: Option<PathBuf>,
        entries: Vec<mcp::config::ServerEntry>,
    ) -> McpManager {
        let bridge = Arc::new(mcp::McpBridge::connect_managed(
            caps.clone(),
            entries.clone(),
        ));
        McpManager {
            caps,
            user_path,
            project_path,
            entries: Mutex::new(entries),
            bridge,
            login: Mutex::new(None),
        }
    }

    /// The bridge handle the registry serves.
    pub fn bridge(&self) -> Arc<mcp::McpBridge> {
        self.bridge.clone()
    }

    /// The entries (for the prompt section).
    pub fn entries(&self) -> Vec<mcp::config::ServerEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Rebuild the bridge around the current entries.
    fn rebuild(&self) {
        let entries = self.entries();
        self.bridge.rebuild(self.caps.clone(), entries);
    }

    /// One status block: every server with state, tools, and source,
    /// then warnings. `/mcp` with no verb prints this.
    pub fn status_text(&self) -> String {
        let statuses = self.bridge.statuses();
        if statuses.is_empty() {
            return "No MCP servers configured. Add one to mcp.json (see the authoring guide)."
                .to_string();
        }
        let mut lines = Vec::with_capacity(statuses.len() + 1);
        for status in &statuses {
            let state = match &status.state {
                mcp::ServerStateKind::Connected => format!("connected ({} tools)", status.tools),
                mcp::ServerStateKind::Disabled => "disabled".to_string(),
                mcp::ServerStateKind::Failed(err) => format!("failed: {err}"),
                mcp::ServerStateKind::NeedsSignIn(detail) => format!("needs sign-in: {detail}"),
            };
            let source = match status.source {
                mcp::config::ServerSource::User => "user",
                mcp::config::ServerSource::Project => "project",
                mcp::config::ServerSource::Inline => "inline",
            };
            let exposure = match status.exposure {
                mcp::config::McpExposure::Codemode => "codemode",
                mcp::config::McpExposure::Deferred => "deferred",
                mcp::config::McpExposure::Direct => "direct",
                mcp::config::McpExposure::Hidden => "hidden",
            };
            lines.push(format!(
                "- {} [{exposure}] ({source}): {state}",
                status.name
            ));
        }
        lines.push(String::new());
        lines.push(
            "Verbs: reconnect <server> · enable <server> · disable <server> · exposure <server> <level> · login <server> · logout <server>".to_string(),
        );
        lines.join("\n")
    }

    /// Run one verb (`/mcp <verb> [target]`), answering with the
    /// notice to show. Unknown verbs list the set.
    pub fn act(&self, verb: &str, target: &str) -> String {
        match verb {
            "" => self.status_text(),
            "reconnect" => self.reconnect(target),
            "enable" => self.set_enabled(target, true),
            "disable" => self.set_enabled(target, false),
            "exposure" => self.set_exposure(target),
            "login" => self.begin_login(target),
            "logout" => self.logout(target),
            _ => format!(
                "unknown /mcp verb {verb:?}; try one of: reconnect, enable, disable, exposure, login, logout"
            ),
        }
    }

    /// Pick one managed entry by name.
    fn find(&self, target: &str) -> Result<mcp::config::ServerEntry, String> {
        self.entries()
            .into_iter()
            .find(|entry| entry.name == target)
            .ok_or_else(|| format!("no MCP server {target:?}"))
    }

    /// Rebuild and report every state (a failure names its server and
    /// cause; the rest keep serving).
    fn reconnect(&self, target: &str) -> String {
        if !target.is_empty() && self.find(target).is_err() {
            return format!("no MCP server {target:?}");
        }
        self.rebuild();
        if target.is_empty() {
            return self.status_text();
        }
        let state = self
            .bridge
            .statuses()
            .into_iter()
            .find(|status| status.name == target);
        match state.map(|status| status.state) {
            Some(mcp::ServerStateKind::Connected) => format!("`{target}` reconnected"),
            Some(mcp::ServerStateKind::Disabled) => format!("`{target}` is disabled"),
            Some(mcp::ServerStateKind::Failed(err)) => format!("`{target}` failed: {err}"),
            Some(mcp::ServerStateKind::NeedsSignIn(detail)) => {
                format!("`{target}` needs sign-in: {detail}")
            }
            None => format!("no MCP server {target:?}"),
        }
    }

    /// Flip one server on or off, persisting the knob (a user-level
    /// server edited under a trusted project writes a project
    /// override, pi's rule; later edits stay in the override).
    fn set_enabled(&self, target: &str, enabled: bool) -> String {
        let mut entry = match self.find(target) {
            Ok(entry) => entry,
            Err(err) => return err,
        };
        entry.enabled = enabled;
        if let Err(err) = self.persist_knobs(&mut entry, enabled, None) {
            return err;
        }
        self.rebuild();
        let word = if enabled { "enabled" } else { "disabled" };
        format!("`{target}` {word}")
    }

    /// Set one server's exposure (`exposure <server> <level>`) or
    /// cycle it direct → codemode → deferred → hidden → direct,
    /// persisting like `set_enabled`.
    fn set_exposure(&self, target: &str) -> String {
        use mcp::config::McpExposure as Exposure;
        let level = |word: &str| match word {
            "direct" => Some(Exposure::Direct),
            "codemode" => Some(Exposure::Codemode),
            "deferred" => Some(Exposure::Deferred),
            "hidden" => Some(Exposure::Hidden),
            _ => None,
        };
        let (name, next) = match target.rsplit_once(' ') {
            Some((name, word)) if level(word).is_some() => (name, level(word)),
            _ => (target, None),
        };
        let mut entry = match self.find(name) {
            Ok(entry) => entry,
            Err(err) => return err,
        };
        let next = next.unwrap_or(match entry.exposure {
            Exposure::Direct => Exposure::Codemode,
            Exposure::Codemode => Exposure::Deferred,
            Exposure::Deferred => Exposure::Hidden,
            Exposure::Hidden => Exposure::Direct,
        });
        entry.exposure = next;
        let enabled = entry.enabled;
        if let Err(err) = self.persist_knobs(&mut entry, enabled, Some(next)) {
            return err;
        }
        self.rebuild();
        let level = match next {
            Exposure::Direct => "direct",
            Exposure::Codemode => "codemode",
            Exposure::Deferred => "deferred",
            Exposure::Hidden => "hidden",
        };
        format!("`{target}` exposure: {level}")
    }

    /// Persist one entry's knobs: the defining file, or a fresh
    /// project override for a user-level server under a trusted
    /// project (later edits stay in the override).
    fn persist_knobs(
        &self,
        entry: &mut mcp::config::ServerEntry,
        enabled: bool,
        exposure: Option<mcp::config::McpExposure>,
    ) -> Result<(), String> {
        let exposure_str = exposure.map(|level| match level {
            mcp::config::McpExposure::Direct => "direct",
            mcp::config::McpExposure::Codemode => "codemode",
            mcp::config::McpExposure::Deferred => "deferred",
            mcp::config::McpExposure::Hidden => "hidden",
        });
        if entry.source == mcp::config::ServerSource::User
            && let Some(project) = &self.project_path
        {
            write_project_override(project, &entry.name, enabled, exposure_str)?;
            entry.source = mcp::config::ServerSource::Project;
        } else {
            let path = match entry.source {
                mcp::config::ServerSource::Project => self.project_path.as_ref(),
                _ => Some(&self.user_path),
            }
            .ok_or_else(|| "no project file to persist to".to_string())?;
            write_full_entry(path, entry)?;
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(known) = entries.iter_mut().find(|known| known.name == entry.name) {
            known.enabled = enabled;
            if let Some(level) = exposure {
                known.exposure = level;
            }
            known.source = entry.source;
        }
        Ok(())
    }

    /// Plan one manager for a session (all through `assemble`):
    /// entries, engine grants, file paths, and the load warnings for
    /// the startup notice. The caller builds the engine (it owns the
    /// prompt plumbing) and hands it to [`McpManager::load`].
    pub fn plan(
        data_dir: &std::path::Path,
        cwd: &std::path::Path,
        trusted: bool,
    ) -> (
        Vec<mcp::config::ServerEntry>,
        lca_tools::CapabilityGrants,
        Vec<String>,
        PathBuf,
        Option<PathBuf>,
    ) {
        let loaded = load_entries(data_dir, cwd, trusted);
        let (grants, mut warnings) = manager_grants(&loaded.entries);
        warnings.extend(loaded.warnings);
        // Trusted whether or not the file exists yet: an override
        // creates it (persistence makes parents).
        let project = trusted.then(|| project_config_path(cwd));
        (
            loaded.entries,
            grants,
            warnings,
            user_config_path(data_dir),
            project,
        )
    }

    /// Start the interactive sign-in on a background thread (the
    /// browser opens where one exists); the result arrives through
    /// [`McpManager::poll`].
    fn begin_login(&self, target: &str) -> String {
        let entry = match self.find(target) {
            Ok(entry) => entry,
            Err(err) => return err,
        };
        let Some(config) = entry.http_config() else {
            return format!("`{target}` is a stdio server: nothing to sign in to");
        };
        if config.oauth.is_none() {
            return format!("`{target}` configures no OAuth");
        }
        let caps = self.caps.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let server = target.to_string();
        std::thread::spawn(move || {
            let outcome = mcp::sign_in(caps, &config);
            let _ = done_tx.send((server, outcome));
        });
        *self
            .login
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(LoginTask {
            server: target.to_string(),
            done: done_rx,
        });
        format!(
            "signing in to `{target}` (a browser page should open; sign-in completes in the background)"
        )
    }

    /// Forget one server's tokens.
    fn logout(&self, target: &str) -> String {
        let entry = match self.find(target) {
            Ok(entry) => entry,
            Err(err) => return err,
        };
        let Some(config) = entry.http_config() else {
            return format!("`{target}` is a stdio server: nothing to sign out of");
        };
        let store = mcp::TokenStore::new(self.caps.clone(), &config);
        match store.clear() {
            Ok(()) => {
                self.rebuild();
                format!("signed out of `{target}`")
            }
            Err(err) => err,
        }
    }

    /// Collect a finished background sign-in (the TUI loop calls
    /// this every tick): rebuild around the fresh tokens and report.
    pub fn poll(&self) -> Option<String> {
        let task = self
            .login
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()?;
        match task.done.try_recv() {
            Ok((server, Ok(()))) => {
                self.rebuild();
                Some(format!("signed in to `{server}`"))
            }
            Ok((server, Err(err))) => Some(format!("sign-in to `{server}` failed: {err}")),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                *self
                    .login
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(task);
                None
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(format!("sign-in to `{}` went away", task.server))
            }
        }
    }
}

/// Read one JSON file (missing reads as empty).
fn read_json_file(path: &Path) -> Result<serde_json::Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|err| format!("{err}")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::json!({"mcpServers": {}}))
        }
        Err(err) => Err(format!("{err}")),
    }
}

/// Write one JSON file, creating parent directories.
fn write_json_file(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{err}"))?;
    }
    let text =
        serde_json::to_string_pretty(value).map_err(|err| format!("cannot encode: {err}"))?;
    std::fs::write(path, text).map_err(|err| format!("{err}"))
}

/// Serialize one entry back to its file shape.
fn entry_json(entry: &mcp::config::ServerEntry) -> serde_json::Value {
    let mut value = serde_json::json!({
        "enabled": entry.enabled,
        "exposure": match entry.exposure {
            mcp::config::McpExposure::Direct => "direct",
            mcp::config::McpExposure::Codemode => "codemode",
            mcp::config::McpExposure::Deferred => "deferred",
            mcp::config::McpExposure::Hidden => "hidden",
        },
    });
    match &entry.kind {
        mcp::config::EntryKind::Stdio {
            command,
            args,
            cwd_scope,
        } => {
            value["command"] = command.clone().into();
            value["args"] = args.clone().into();
            value["cwd"] = cwd_scope.clone().into();
        }
        mcp::config::EntryKind::Http {
            url,
            headers,
            timeout_secs,
            oauth,
        } => {
            value["url"] = url.clone().into();
            value["headers"] = headers
                .iter()
                .map(|(key, header)| (key.clone(), header.clone().into()))
                .collect::<serde_json::Map<String, serde_json::Value>>()
                .into();
            if *timeout_secs > 0 {
                value["timeout"] = (*timeout_secs).into();
            }
            if let Some(oauth) = oauth {
                let mut object = serde_json::json!({});
                if let Some(name) = &oauth.client_name {
                    object["clientName"] = name.clone().into();
                }
                if let Some(scope) = &oauth.scope {
                    object["scope"] = scope.clone().into();
                }
                if let Some(url) = &oauth.auth_server_metadata_url {
                    object["authServerMetadataUrl"] = url.clone().into();
                }
                if let Some(id) = &oauth.client_id {
                    object["clientId"] = id.clone().into();
                }
                value["oauth"] = object;
            }
        }
    }
    if !entry.tool_exposure.is_empty() {
        let mut rules = serde_json::Map::new();
        for (tool, exposure) in &entry.tool_exposure {
            rules.insert(
                tool.clone(),
                match exposure {
                    mcp::config::McpExposure::Direct => "direct",
                    mcp::config::McpExposure::Codemode => "codemode",
                    mcp::config::McpExposure::Deferred => "deferred",
                    mcp::config::McpExposure::Hidden => "hidden",
                }
                .into(),
            );
        }
        value["toolExposure"] = rules.into();
    }
    if !entry.description.is_empty() {
        value["description"] = entry.description.clone().into();
    }
    value
}

/// Write one full entry into its file, keeping every other server
/// and every unknown key.
fn write_full_entry(path: &Path, entry: &mcp::config::ServerEntry) -> Result<(), String> {
    let mut file = read_json_file(path)?;
    if !file.is_object() {
        file = serde_json::json!({"mcpServers": {}});
    }
    if file.get("mcpServers").is_none() {
        file["mcpServers"] = serde_json::json!({});
    }
    file["mcpServers"][&entry.name] = entry_json(entry);
    write_json_file(path, &file)
}

/// Write a knob-only project override (a user-level server edited
/// under a trusted project), merging with whatever the file holds.
fn write_project_override(
    path: &Path,
    name: &str,
    enabled: bool,
    exposure: Option<&str>,
) -> Result<(), String> {
    let mut file = read_json_file(path)?;
    if !file.is_object() {
        file = serde_json::json!({"mcpServers": {}});
    }
    if file.get("mcpServers").is_none() {
        file["mcpServers"] = serde_json::json!({});
    }
    let mut patch = serde_json::json!({"enabled": enabled});
    if let Some(level) = exposure {
        patch["exposure"] = level.into();
    }
    // Merge with an existing override instead of clobbering it.
    let patch_object: Vec<(String, serde_json::Value)> =
        patch.as_object().map_or_else(Vec::new, |object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        });
    if let Some(existing) = file["mcpServers"]
        .get(name)
        .and_then(|entry| entry.as_object())
    {
        let mut merged = existing.clone();
        for (key, value) in patch_object {
            merged.insert(key, value);
        }
        file["mcpServers"][name] = merged.into();
    } else {
        file["mcpServers"][name] = patch;
    }
    write_json_file(path, &file)
}

/// The `## MCP servers` section: configured, non-direct servers with
/// one line each on how their tools are reached (pi lists exactly
/// these; direct tools are declared anyway, hidden and disabled stay
/// silent).
pub fn servers_section(entries: &[mcp::config::ServerEntry]) -> Option<String> {
    let mut lines = Vec::new();
    for entry in entries {
        if !entry.enabled {
            continue;
        }
        let reach = match entry.exposure {
            mcp::config::McpExposure::Direct | mcp::config::McpExposure::Hidden => continue,
            mcp::config::McpExposure::Codemode => "callable from scripts; find them by server name",
            mcp::config::McpExposure::Deferred => "load with tool_search, then call directly",
        };
        let what = if entry.description.is_empty() {
            entry.name.clone()
        } else {
            format!("{}: {}", entry.name, entry.description)
        };
        lines.push(format!("- {what} ({reach})."));
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}
