//! The MCP session manager (gh #53 phase 3, folded here by gh #418 Phase B):
//! lifecycle state, capability-grant assembly, the `/mcp` action dispatch, and
//! the `## MCP servers` system-prompt section.
//!
//! This module is UI-free by design. It answers with a notice string and never
//! touches a widget, a terminal, or the agent loop - the host renders whatever
//! comes back. File locations and `mcp.json` persistence live in
//! [`crate::config`], because that module already owns the parser.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::config::{
    EntryKind, McpExposure, ServerEntry, ServerSource, load_entries, project_config_path,
    user_config_path, write_full_entry, write_project_override,
};

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
pub fn manager_grants(entries: &[ServerEntry]) -> (lca_tools::CapabilityGrants, Vec<String>) {
    let mut warnings = Vec::new();
    let mut grants = crate::manifest_grants();
    for entry in entries {
        let EntryKind::Http { url, .. } = &entry.kind else {
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
    // gh #418 Phase D: a metadata document the user *configured* names a host
    // the sign-in will legitimately need (`authServerMetadataUrl`, pi's
    // `authServerMetadataUrl`). Configuration is the consent, exactly as it is
    // for the server's own URL above, so it joins the static set. A host that
    // is only *discovered* stays flow-scoped instead, because the user never
    // named it.
    for entry in entries {
        let configured_urls = entry
            .http_config()
            .and_then(|config| config.oauth)
            .and_then(|oauth| oauth.auth_server_metadata_url);
        let Some(url) = configured_urls else {
            continue;
        };
        let Some(host) = url_host(&url) else {
            warnings.push(format!(
                "server {:?} configures an OAuth metadata URL with no usable host",
                entry.name
            ));
            continue;
        };
        match lca_permissions::parse_net_pattern(&host) {
            Ok(pattern) => {
                if !grants.adhoc_net.contains(&pattern) {
                    grants.adhoc_net.push(pattern);
                }
            }
            Err(err) => warnings.push(format!(
                "server {:?} grants nothing for its OAuth metadata host ({}); sign-in will deny",
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
    entries: Mutex<Vec<ServerEntry>>,
    bridge: Arc<crate::McpBridge>,
    login: Mutex<Option<LoginTask>>,
    /// Rows finished by background workers, waiting to land (gh #418 Phase E).
    /// Behind a mutex because `McpManager` is shared across threads
    /// (`Receiver` is `Send` but not `Sync`), and only the draining methods
    /// touch it.
    landed: Mutex<std::sync::mpsc::Receiver<crate::native::ManagedServer>>,
    /// A sender for the same channel, so a rebuild can start a new round.
    landed_tx: std::sync::mpsc::Sender<crate::native::ManagedServer>,
    /// Whether a turn has already waited for the startup connections since the
    /// session started (pi's `waitedForStartup`). Reset by every rebuild.
    waited_for_startup: std::sync::atomic::AtomicBool,
}

/// pi's `DEFAULT_STARTUP_WAIT_MS`: how long the first turn waits for servers
/// whose tools are declared to the model.
pub const DEFAULT_STARTUP_WAIT_MS: std::time::Duration = std::time::Duration::from_millis(10_000);

impl McpManager {
    /// The sign-in screen's copy (gh #418 Phase D's consent property): when a
    /// sign-in discovered an authorization host outside the MCP server's own,
    /// it is named here, so the user sees where the credential went.
    ///
    /// Empty when nothing has been discovered outside, which is the common
    /// case and means there is nothing to say.
    pub fn discovery_note(&self, server: &str) -> Option<String> {
        let entry = self
            .entries()
            .into_iter()
            .find(|entry| entry.name == server)?;
        let config = entry.http_config()?;
        crate::TokenStore::new(self.caps.clone(), &config)
            .discovery_note()
            .ok()
            .flatten()
    }

    /// Load, grant, and connect (all through `assemble`).
    pub fn load(
        caps: Arc<lca_tools::Capabilities>,
        user_path: PathBuf,
        project_path: Option<PathBuf>,
        entries: Vec<ServerEntry>,
    ) -> McpManager {
        // gh #418 Phase E: connect in the background. `registry::assemble`
        // must return inside NFR-3's budget whether a server is slow, hung,
        // or unreachable; a synchronous connect made cold start the sum of
        // every server's timeout.
        let (done, landed) = std::sync::mpsc::channel();
        let landed_tx_for_manager = done.clone();
        let bridge = Arc::new(crate::McpBridge::spawn_connections(
            caps.clone(),
            entries.clone(),
            done,
        ));
        McpManager {
            caps,
            user_path,
            project_path,
            entries: Mutex::new(entries),
            bridge,
            login: Mutex::new(None),
            landed: Mutex::new(landed),
            landed_tx: landed_tx_for_manager,
            waited_for_startup: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Land every row a background worker has finished, without blocking.
    ///
    /// Called from the places that read state (`status_text`, `poll`,
    /// `wait_for_startup`) so a finished connection becomes visible the next
    /// time anyone looks, and from the turn boundary so the first request
    /// sees the tools a fast server already produced.
    pub fn drain_landed(&self) {
        // Hold the receiver lock only while draining, never across `land`
        // (which takes the bridge's own lock).
        let mut rows = Vec::new();
        {
            let receiver = self
                .landed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while let Ok(row) = receiver.try_recv() {
                rows.push(row);
            }
        }
        for row in rows {
            self.bridge.land(row);
        }
    }

    /// Wait until every background connection has finished (or the budget
    /// runs out), landing results as they arrive.
    ///
    /// The tests' seam, and the honest one for any caller that wants the table
    /// settled before reading it: `/mcp` and the turn boundary use
    /// [`Self::wait_for_startup`] instead, which waits only for what a turn
    /// actually needs.
    pub fn wait_until_settled(&self, budget: std::time::Duration) -> Vec<String> {
        let deadline = std::time::Instant::now() + budget;
        loop {
            self.drain_landed();
            let pending = self.bridge.connecting_names();
            if pending.is_empty() {
                return Vec::new();
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return pending;
            }
            std::thread::sleep((deadline - now).min(std::time::Duration::from_millis(10)));
        }
    }

    /// Wait (bounded) for the connections a turn needs, then land them.
    ///
    /// pi's rule (`index.ts`, `waitForDirectServers`): the **first** turn
    /// waits up to [`DEFAULT_STARTUP_WAIT_MS`] for enabled servers whose tools
    /// are declared straight to the model, so they appear in the first
    /// request; servers behind `tool_search`, codemode, or the resource tools
    /// are waited for on demand instead, and a hung server never holds a turn
    /// past the budget.
    ///
    /// Returns the names still connecting when the budget ran out, so the host
    /// can say so instead of silently omitting them.
    pub fn wait_for_startup(&self) -> Vec<String> {
        self.wait_for_startup_with(DEFAULT_STARTUP_WAIT_MS)
    }

    /// [`Self::wait_for_startup`] with an explicit budget (the tests use a
    /// short one so a hung server costs a second rather than ten).
    pub fn wait_for_startup_with(&self, budget: std::time::Duration) -> Vec<String> {
        use std::sync::atomic::Ordering;
        if self.waited_for_startup.swap(true, Ordering::SeqCst) {
            // A later turn does not wait again: either the servers landed or
            // they are still coming, and the model's view settles next turn.
            self.drain_landed();
            return self.bridge.connecting_names();
        }
        self.drain_landed();
        // Only the servers whose tools are declared to the model justify
        // holding a turn: everything else is reachable through discovery,
        // which waits on demand.
        let declaring = self
            .entries()
            .into_iter()
            .filter(|entry| entry.enabled && entry.exposure == McpExposure::Direct)
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        if declaring.is_empty() {
            return Vec::new();
        }
        let deadline = std::time::Instant::now() + budget;
        loop {
            self.drain_landed();
            let pending: Vec<String> = self
                .bridge
                .connecting_names()
                .into_iter()
                .filter(|name| declaring.contains(name))
                .collect();
            if pending.is_empty() {
                return Vec::new();
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return pending;
            }
            // A short poll keeps the wait responsive to a worker that lands
            // mid-sleep, without a channel select on one receiver per server.
            std::thread::sleep((deadline - now).min(std::time::Duration::from_millis(10)));
        }
    }

    /// Forget the startup wait, so a fresh connection round waits again (the
    /// edit paths call this: a reconnected direct server should be declared to
    /// the next request, not silently missing).
    fn reset_startup_wait(&self) {
        self.waited_for_startup
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// The bridge handle the registry serves.
    pub fn bridge(&self) -> Arc<crate::McpBridge> {
        self.bridge.clone()
    }

    /// The entries (for the prompt section).
    pub fn entries(&self) -> Vec<ServerEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Rebuild the bridge around the current entries.
    fn rebuild(&self) {
        let entries = self.entries();
        // Land anything a previous round finished before the table is
        // replaced; a row from the old table must never resurrect.
        self.drain_landed();
        self.bridge
            .rebuild_connecting(self.caps.clone(), entries, self.landed_tx());
        // A fresh round can be waited for again: a reconnected `direct`
        // server should be declared to the next request.
        self.reset_startup_wait();
    }

    /// A sender that feeds this manager's landing channel.
    ///
    /// The channel is owned by the manager (one receiver), so a rebuild needs
    /// a second sender rather than a new channel - a new channel would orphan
    /// the workers the previous one still has.
    fn landed_tx(&self) -> std::sync::mpsc::Sender<crate::native::ManagedServer> {
        self.landed_tx.clone()
    }

    /// One status block: every server with state, tools, and source,
    /// then warnings. `/mcp` with no verb prints this.
    pub fn status_text(&self) -> String {
        // A server that landed since the last look shows as connected now.
        self.drain_landed();
        let statuses = self.bridge.statuses();
        if statuses.is_empty() {
            return "No MCP servers configured. Add one to mcp.json (see the authoring guide)."
                .to_string();
        }
        let mut lines = Vec::with_capacity(statuses.len() + 1);
        for status in &statuses {
            let state = match &status.state {
                crate::ServerStateKind::Connected => {
                    format!("connected ({} tools)", status.tools)
                }
                crate::ServerStateKind::Disabled => "disabled".to_string(),
                // gh #418 Phase E: honest, not silent. A server still
                // connecting when `/mcp` is opened says so.
                crate::ServerStateKind::Connecting => "connecting".to_string(),
                crate::ServerStateKind::Failed(err) => format!("failed: {err}"),
                crate::ServerStateKind::NeedsSignIn(detail) => format!("needs sign-in: {detail}"),
            };
            let source = match status.source {
                ServerSource::User => "user",
                ServerSource::Project => "project",
                ServerSource::Inline => "inline",
            };
            let exposure = match status.exposure {
                McpExposure::Codemode => "codemode",
                McpExposure::Deferred => "deferred",
                McpExposure::Direct => "direct",
                McpExposure::Hidden => "hidden",
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
    fn find(&self, target: &str) -> Result<ServerEntry, String> {
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
            Some(crate::ServerStateKind::Connected) => format!("`{target}` reconnected"),
            Some(crate::ServerStateKind::Disabled) => format!("`{target}` is disabled"),
            // gh #418 Phase E: a reconnect starts a background round, so the
            // honest verb is "reconnecting" - the server it just restarted has
            // not landed yet. Reporting "reconnected" here would claim a
            // connection the user does not have.
            Some(crate::ServerStateKind::Connecting) => {
                format!("`{target}` reconnecting (its tools appear once it lands)")
            }
            Some(crate::ServerStateKind::Failed(err)) => format!("`{target}` failed: {err}"),
            Some(crate::ServerStateKind::NeedsSignIn(detail)) => {
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
        use McpExposure as Exposure;
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
        // gh #418 Phase C#C4 (gh #267): name the server, not the raw
        // argument - `exposure echo direct` used to reply with the whole
        // `echo direct` string in backticks.
        format!("`{name}` exposure: {level}")
    }

    /// Persist one entry's knobs: the defining file, or a fresh
    /// project override for a user-level server under a trusted
    /// project (later edits stay in the override).
    fn persist_knobs(
        &self,
        entry: &mut ServerEntry,
        enabled: bool,
        exposure: Option<McpExposure>,
    ) -> Result<(), String> {
        let exposure_str = exposure.map(|level| match level {
            McpExposure::Direct => "direct",
            McpExposure::Codemode => "codemode",
            McpExposure::Deferred => "deferred",
            McpExposure::Hidden => "hidden",
        });
        if entry.source == ServerSource::User
            && let Some(project) = &self.project_path
        {
            write_project_override(project, &entry.name, enabled, exposure_str)?;
            entry.source = ServerSource::Project;
        } else {
            let path = match entry.source {
                ServerSource::Project => self.project_path.as_ref(),
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
        Vec<ServerEntry>,
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
            let outcome = crate::sign_in(caps, &config);
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
        let store = crate::TokenStore::new(self.caps.clone(), &config);
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
        self.drain_landed();
        let task = self
            .login
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()?;
        match task.done.try_recv() {
            Ok((server, Ok(()))) => {
                self.rebuild();
                // Name a discovered outside host, so the report says where the
                // credential went rather than only that it went (gh #418).
                match self.discovery_note(&server) {
                    Some(note) => Some(format!("signed in to `{server}` ({note})")),
                    None => Some(format!("signed in to `{server}`")),
                }
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

/// The `## MCP servers` section: configured, non-direct servers with
/// one line each on how their tools are reached (pi lists exactly
/// these; direct tools are declared anyway, hidden and disabled stay
/// silent).
pub fn servers_section(entries: &[ServerEntry]) -> Option<String> {
    let mut lines = Vec::new();
    for entry in entries {
        if !entry.enabled {
            continue;
        }
        let reach = match entry.exposure {
            McpExposure::Direct | McpExposure::Hidden => continue,
            McpExposure::Codemode => "callable from scripts; find them by server name",
            McpExposure::Deferred => "load with tool_search, then call directly",
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
