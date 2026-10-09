//! The in-process twin: spawns each server through the shared
//! [`Capabilities`](lca_tools::Capabilities) engine and serves the
//! `tool-catalog` world from the live connections.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::{ServerConfig, qualify_tools};

/// The grants the manifest declares: `process` to spawn the servers,
/// `fs` (workspace, read) for the scope they run in. A test keeps them
/// in step with `extension.toml`.
#[allow(clippy::expect_used)] // the extension's own literal manifest patterns are valid by construction.
pub fn manifest_grants() -> lca_tools::CapabilityGrants {
    lca_tools::CapabilityGrants {
        process: true,
        fs: vec![
            lca_permissions::ScopeGrant::parse("workspace", lca_permissions::FsMode::Read)
                .expect("workspace read grant parses"),
        ],
        fs_declared: true,
        ..Default::default()
    }
}

/// [`Stdio`](crate::Stdio) over one spawned child.
struct ChildPipe {
    caps: Arc<lca_tools::Capabilities>,
    handle: u32,
}

impl crate::Stdio for ChildPipe {
    fn write_all(&self, bytes: &[u8]) -> Result<(), String> {
        self.caps
            .process_write_stdin(self.handle, bytes)
            .map(|_| ())
            .map_err(|err| format!("MCP server write failed: {err}"))
    }

    fn read_some(&self, max: usize) -> Result<Option<Vec<u8>>, String> {
        self.caps
            .process_read_stdout(self.handle, max)
            .map_err(|err| format!("MCP server read failed: {err}"))
    }
}

/// One live server: a stdio child or a remote endpoint. Both serve
/// the same spec and call shapes; only the pipe differs.
enum LiveServer {
    Stdio {
        caps: Arc<lca_tools::Capabilities>,
        handle: u32,
        /// One JSON-RPC exchange at a time per server, which is also
        /// what keeps response ids matched to their requests.
        session: Mutex<crate::Session<ChildPipe>>,
        specs: Vec<lca_protocol::ToolSpec>,
        /// Qualified name back to the server-side tool name.
        calls: BTreeMap<String, String>,
        /// The entry's exposure (resource-tool width included).
        exposure: lca_protocol::ToolExposure,
        /// Whether the handshake offered resources.
        resources: bool,
    },
    Http {
        session: Mutex<crate::remote::HttpSession>,
        specs: Vec<lca_protocol::ToolSpec>,
        /// Qualified name back to the server-side tool name.
        calls: BTreeMap<String, String>,
        /// The entry's exposure (resource-tool width included).
        exposure: lca_protocol::ToolExposure,
        /// Whether the handshake offered resources.
        resources: bool,
    },
}

impl LiveServer {
    fn shutdown(&self) {
        if let LiveServer::Stdio { caps, handle, .. } = self {
            let _ = caps.process_kill(*handle);
        }
    }

    fn specs(&self) -> &[lca_protocol::ToolSpec] {
        match self {
            LiveServer::Stdio { specs, .. } | LiveServer::Http { specs, .. } => specs,
        }
    }

    fn calls(&self) -> &BTreeMap<String, String> {
        match self {
            LiveServer::Stdio { calls, .. } | LiveServer::Http { calls, .. } => calls,
        }
    }

    fn exposure(&self) -> lca_protocol::ToolExposure {
        match self {
            LiveServer::Stdio { exposure, .. } | LiveServer::Http { exposure, .. } => *exposure,
        }
    }

    fn offers_resources(&self) -> bool {
        match self {
            LiveServer::Stdio { resources, .. } | LiveServer::Http { resources, .. } => *resources,
        }
    }

    fn locked<T>(
        &self,
        run: impl FnOnce(&mut LockedSession<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        let poisoned = || "MCP session lock is poisoned".to_string();
        match self {
            LiveServer::Stdio { session, .. } => {
                let guard = session.lock().map_err(|_| poisoned())?;
                run(&mut LockedSession::Stdio(guard))
            }
            LiveServer::Http { session, .. } => {
                let guard = session.lock().map_err(|_| poisoned())?;
                run(&mut LockedSession::Http(guard))
            }
        }
    }

    fn resource_list(
        &self,
        cursor: Option<&str>,
    ) -> Result<(Vec<crate::McpResource>, Option<String>), String> {
        self.locked(|session| session.resource_list(cursor))
    }

    fn resource_templates(&self) -> Result<Vec<crate::McpResourceTemplate>, String> {
        self.locked(|session| session.resource_templates())
    }

    fn resource_read(&self, uri: &str) -> Result<Vec<crate::ResourceContent>, String> {
        self.locked(|session| session.resource_read(uri))
    }
}

/// One locked session behind either transport.
enum LockedSession<'a> {
    Stdio(std::sync::MutexGuard<'a, crate::Session<ChildPipe>>),
    Http(std::sync::MutexGuard<'a, crate::remote::HttpSession>),
}

impl LockedSession<'_> {
    fn resource_list(
        &mut self,
        cursor: Option<&str>,
    ) -> Result<(Vec<crate::McpResource>, Option<String>), String> {
        match self {
            LockedSession::Stdio(session) => session.resource_list(cursor),
            LockedSession::Http(session) => session.resource_list(cursor),
        }
    }

    fn resource_templates(&mut self) -> Result<Vec<crate::McpResourceTemplate>, String> {
        match self {
            LockedSession::Stdio(session) => session.resource_templates(),
            LockedSession::Http(session) => session.resource_templates(),
        }
    }

    fn resource_read(&mut self, uri: &str) -> Result<Vec<crate::ResourceContent>, String> {
        match self {
            LockedSession::Stdio(session) => session.resource_read(uri),
            LockedSession::Http(session) => session.resource_read(uri),
        }
    }
}

/// One `list_mcp_resources` row (pi's shape, plus the server).
fn resource_row(server: &str, resource: &crate::McpResource) -> serde_json::Value {
    let mut row = serde_json::json!({
        "server": server,
        "uri": resource.uri,
        "name": resource.name,
    });
    if let Some(title) = &resource.title {
        row["title"] = title.clone().into();
    }
    if let Some(description) = &resource.description {
        row["description"] = description.clone().into();
    }
    if let Some(mime) = &resource.mime_type {
        row["mimeType"] = mime.clone().into();
    }
    row
}

/// Map read contents to one result: text stays text, images ride
/// the image lane, other bytes land in a file whose path the model
/// receives (pi's rule).
fn read_result(
    call_id: &str,
    server: &str,
    contents: Vec<crate::ResourceContent>,
) -> Result<lca_protocol::ToolResult, String> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut text = String::new();
    let mut images = Vec::new();
    for content in contents {
        match content {
            crate::ResourceContent::Text { text: part, .. } => text.push_str(&part),
            crate::ResourceContent::Blob {
                uri,
                mime_type,
                bytes,
            } => {
                let image = mime_type
                    .as_deref()
                    .is_some_and(|mime| mime.starts_with("image/"));
                if image {
                    images.push(lca_protocol::ImageContent {
                        media_type: mime_type.unwrap_or_else(|| "image/png".to_string()),
                        bytes,
                    });
                    continue;
                }
                let dir = std::env::temp_dir().join("lca-mcp-resources");
                std::fs::create_dir_all(&dir)
                    .map_err(|err| format!("cannot stage the resource: {err}"))?;
                let leaf = uri.rsplit('/').next().unwrap_or("resource");
                let safe: String = leaf
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let path = dir.join(format!("{server}-{id}-{safe}"));
                std::fs::write(&path, &bytes)
                    .map_err(|err| format!("cannot stage the resource: {err}"))?;
                text.push_str(&path.to_string_lossy());
            }
        }
    }
    let mut result = lca_protocol::ToolResult::ok(call_id.to_string(), text);
    result.images = images;
    Ok(result)
}

/// What one managed server is doing (the `/mcp` rows read these).
#[derive(Debug, Clone)]
pub struct ServerStatus {
    /// pi's server name.
    pub name: String,
    /// How the model reaches its tools.
    pub exposure: crate::config::McpExposure,
    /// Which file defined it (edits persist there, or to the
    /// project override).
    pub source: crate::config::ServerSource,
    /// Whether it is switched on.
    pub enabled: bool,
    /// The connection state.
    pub state: ServerStateKind,
    /// Live tools right now (zero unless connected).
    pub tools: usize,
}

/// One managed server's connection state.
#[derive(Debug, Clone)]
pub enum ServerStateKind {
    /// Handshook and listed.
    Connected,
    /// Kept without connecting.
    Disabled,
    /// The handshake failed; the string names the cause.
    Failed(String),
    /// The server wants OAuth; the string says which scopes.
    NeedsSignIn(String),
}

/// One managed server: its entry, its live session when connected,
/// and its state for the manager.
struct ManagedServer {
    entry: crate::config::ServerEntry,
    live: Option<LiveServer>,
    state: ServerStateKind,
}

impl ManagedServer {
    fn status(&self) -> ServerStatus {
        ServerStatus {
            name: self.entry.name.clone(),
            exposure: self.entry.exposure,
            source: self.entry.source,
            enabled: self.entry.enabled,
            state: self.state.clone(),
            tools: self
                .live
                .as_ref()
                .map(|live| live.specs().len())
                .unwrap_or(0),
        }
    }

    fn shutdown(&mut self) {
        if let Some(live) = self.live.take() {
            live.shutdown();
        }
    }
}

/// The bridge: one long-lived session per connected server, plus a
/// state row for every managed entry (connected or not). The rows sit
/// behind a lock so the manager can rebuild in place: the registry
/// keeps one handle and later turns see the new table with no
/// re-registration.
pub struct McpBridge {
    servers: Mutex<Vec<ManagedServer>>,
}

/// Connect one stdio entry: spawn (the permission layer asks),
/// handshake, list, qualify with the entry's exposure.
fn connect_one_stdio(
    caps: &Arc<lca_tools::Capabilities>,
    entry: &crate::config::ServerEntry,
    config: &ServerConfig,
) -> Result<LiveServer, String> {
    let handle = caps
        .process_spawn(&config.command, &config.args, &config.cwd_scope)
        .map_err(|err| format!("MCP server {:?} did not start: {err}", entry.name))?;
    let outcome = (|| -> Result<LiveServer, String> {
        let mut session = crate::Session::new(ChildPipe {
            caps: caps.clone(),
            handle,
        });
        let offers = session.initialize()?;
        let listed = session.tools(&entry.name)?;
        let calls = listed
            .iter()
            .map(|tool| (tool.qualified.clone(), tool.name.clone()))
            .collect();
        let specs = qualify_tools(&entry.name, listed, entry.exposure, &entry.tool_exposure)?;
        Ok(LiveServer::Stdio {
            caps: caps.clone(),
            handle,
            session: Mutex::new(session),
            specs,
            calls,
            exposure: entry.exposure.as_tool_exposure(),
            resources: offers,
        })
    })();
    match outcome {
        Ok(live) => Ok(live),
        Err(err) => {
            let _ = caps.process_kill(handle);
            Err(format!("MCP server {:?} failed: {err}", entry.name))
        }
    }
}

/// Connect one remote entry, sorting the outcome into live, silent
/// sign-in, or failure (a 401 is a state, not a transport error).
fn connect_one_http(
    caps: &Arc<lca_tools::Capabilities>,
    entry: &crate::config::ServerEntry,
    config: &crate::HttpServerConfig,
) -> Result<LiveServer, ServerStateKind> {
    let mut session = crate::remote::HttpSession::new(caps.clone(), config.clone());
    let offers = match session.initialize() {
        Ok(offers) => offers,
        Err(err) => {
            return Err(if err.contains(crate::remote::NEEDS_AUTH_MARKER) {
                ServerStateKind::NeedsSignIn(err)
            } else {
                ServerStateKind::Failed(err)
            });
        }
    };
    let listed = session
        .tools(&entry.name)
        .map_err(ServerStateKind::Failed)?;
    let calls = listed
        .iter()
        .map(|tool| (tool.qualified.clone(), tool.name.clone()))
        .collect();
    let specs = qualify_tools(&entry.name, listed, entry.exposure, &entry.tool_exposure)
        .map_err(ServerStateKind::Failed)?;
    Ok(LiveServer::Http {
        session: Mutex::new(session),
        specs,
        calls,
        exposure: entry.exposure.as_tool_exposure(),
        resources: offers,
    })
}

/// Wrap one inline stdio config in a default entry (the phase-1
/// constructor: direct, enabled, never persisted).
fn inline_stdio(config: ServerConfig) -> crate::config::ServerEntry {
    crate::config::ServerEntry {
        name: config.name.clone(),
        kind: crate::config::EntryKind::Stdio {
            command: config.command.clone(),
            args: config.args.clone(),
            cwd_scope: config.cwd_scope.clone(),
        },
        enabled: true,
        exposure: crate::config::McpExposure::Direct,
        tool_exposure: Vec::new(),
        description: String::new(),
        source: crate::config::ServerSource::Inline,
    }
}

/// Wrap one inline HTTP config in a default entry (the phase-2
/// constructor).
fn inline_http(config: crate::HttpServerConfig) -> crate::config::ServerEntry {
    crate::config::ServerEntry {
        name: config.name.clone(),
        kind: crate::config::EntryKind::Http {
            url: config.url.clone(),
            headers: config.headers.clone(),
            timeout_secs: config.timeout_secs,
            oauth: config.oauth.clone(),
        },
        enabled: true,
        exposure: crate::config::McpExposure::Direct,
        tool_exposure: Vec::new(),
        description: String::new(),
        source: crate::config::ServerSource::Inline,
    }
}

impl McpBridge {
    /// Connect managed entries: every enabled server is tried, and
    /// every entry lands a state row (connected, disabled, failed, or
    /// needs-sign-in). One server's failure never blocks the rest;
    /// the strict constructors below are the all-or-nothing twins.
    pub fn connect_managed(
        caps: Arc<lca_tools::Capabilities>,
        entries: Vec<crate::config::ServerEntry>,
    ) -> McpBridge {
        let mut servers = Vec::with_capacity(entries.len());
        for entry in entries {
            if !entry.enabled {
                servers.push(ManagedServer {
                    entry,
                    live: None,
                    state: ServerStateKind::Disabled,
                });
                continue;
            }
            match &entry.kind {
                crate::config::EntryKind::Stdio { .. } => {
                    let Some(config) = entry.stdio_config() else {
                        continue;
                    };
                    match connect_one_stdio(&caps, &entry, &config) {
                        Ok(live) => servers.push(ManagedServer {
                            entry,
                            live: Some(live),
                            state: ServerStateKind::Connected,
                        }),
                        Err(err) => servers.push(ManagedServer {
                            entry,
                            live: None,
                            state: ServerStateKind::Failed(err),
                        }),
                    }
                }
                crate::config::EntryKind::Http { .. } => {
                    let Some(config) = entry.http_config() else {
                        continue;
                    };
                    match connect_one_http(&caps, &entry, &config) {
                        Ok(live) => servers.push(ManagedServer {
                            entry,
                            live: Some(live),
                            state: ServerStateKind::Connected,
                        }),
                        Err(state) => servers.push(ManagedServer {
                            entry,
                            live: None,
                            state,
                        }),
                    }
                }
            }
        }
        McpBridge {
            servers: Mutex::new(servers),
        }
    }

    /// Reconnect every managed entry in place (the manager calls
    /// this after any edit): old sessions shut down, states refresh,
    /// and the same handle serves the new table.
    pub fn rebuild(
        &self,
        caps: Arc<lca_tools::Capabilities>,
        entries: Vec<crate::config::ServerEntry>,
    ) {
        let mut servers = self
            .servers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for server in servers.iter_mut() {
            server.shutdown();
        }
        *servers = McpBridge::connect_managed(caps, entries).take_servers();
    }

    /// Take the rows out (rebuild plumbing only).
    fn take_servers(self) -> Vec<ManagedServer> {
        self.servers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .collect()
    }

    /// Spawn every server (each spawn asks the permission layer),
    /// handshake, and list tools. A refusal stops the bridge: a server
    /// the user declined never starts, and the denial is recorded on
    /// the engine by the spawn itself.
    pub fn connect(
        caps: Arc<lca_tools::Capabilities>,
        servers: Vec<ServerConfig>,
    ) -> Result<McpBridge, String> {
        let entries = servers.into_iter().map(inline_stdio).collect::<Vec<_>>();
        let bridge = McpBridge::connect_managed(caps, entries);
        for server in bridge.locked().iter() {
            if let ServerStateKind::Failed(err) = &server.state {
                return Err(err.clone());
            }
        }
        Ok(bridge)
    }

    /// Connect every remote server over streamable HTTP (each
    /// handshake asks nothing: authentication rides stored tokens,
    /// and a 401 reports that sign-in is needed). A failure stops
    /// the bridge with the server and the cause named.
    pub fn connect_http(
        caps: Arc<lca_tools::Capabilities>,
        servers: Vec<crate::HttpServerConfig>,
    ) -> Result<McpBridge, String> {
        let entries = servers.into_iter().map(inline_http).collect::<Vec<_>>();
        let bridge = McpBridge::connect_managed(caps, entries);
        for server in bridge.locked().iter() {
            match &server.state {
                ServerStateKind::Failed(err) | ServerStateKind::NeedsSignIn(err) => {
                    return Err(err.clone());
                }
                ServerStateKind::Connected | ServerStateKind::Disabled => {}
            }
        }
        Ok(bridge)
    }

    /// One state row per managed entry, in entry order (the `/mcp`
    /// rows read these).
    /// Lock the rows for one read (poisoning recovers: a dead
    /// manager thread must not wedge the turn).
    fn locked(&self) -> std::sync::MutexGuard<'_, Vec<ManagedServer>> {
        self.servers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// One state row per managed entry, in entry order (the `/mcp`
    /// rows read these).
    pub fn statuses(&self) -> Vec<ServerStatus> {
        self.locked().iter().map(ManagedServer::status).collect()
    }

    /// The servers offering resources, in entry order (borrows
    /// the locked rows the caller holds).
    fn resource_servers(servers: &[ManagedServer]) -> Vec<(String, &LiveServer)> {
        servers
            .iter()
            .filter_map(|server| {
                server.live.as_ref().and_then(|live| {
                    live.offers_resources()
                        .then(|| (server.entry.name.clone(), live))
                })
            })
            .collect()
    }

    /// Every bridged tool, sorted (the registry's stable order):
    /// server tools plus the resource trio when a connected server
    /// offers resources, at the widest such server's exposure.
    pub fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, lca_protocol::DispatchError> {
        let mut specs = Vec::new();
        for server in self.locked().iter() {
            if let Some(live) = &server.live {
                specs.extend(live.specs().iter().cloned());
            }
        }
        let locked = self.locked();
        let exposures = Self::resource_servers(&locked)
            .iter()
            .map(|(_, live)| live.exposure())
            .collect::<Vec<_>>();
        if let Some(exposure) = crate::widest_resource_exposure(&exposures) {
            specs.extend(crate::resource_tool_specs(exposure));
        }
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(specs)
    }

    /// Run one resource call (the trio routes here before the server
    /// table: the tools belong to the bridge, not to one server).
    fn execute_resource(
        &self,
        call: &lca_protocol::ToolCall,
    ) -> Result<lca_protocol::ToolResult, lca_protocol::DispatchError> {
        let failed = |detail: String| lca_protocol::DispatchError::Failed(detail);
        let args: serde_json::Value = serde_json::from_str(&call.arguments)
            .map_err(|err| lca_protocol::DispatchError::Failed(format!("bad arguments: {err}")))?;
        let text = |key: &str| {
            args.get(key)
                .and_then(|value| value.as_str())
                .map(str::to_string)
        };
        match call.name.as_str() {
            crate::LIST_RESOURCES_TOOL => {
                let (server, cursor) = match (text("server"), text("cursor")) {
                    (_, Some(cursor)) => cursor
                        .split_once(':')
                        .map(|(server, page)| (Some(server.to_string()), Some(page.to_string())))
                        .ok_or_else(|| failed("bad cursor".to_string()))?,
                    (server, None) => (server, None),
                };
                let mut rows = Vec::new();
                let mut next = None;
                let locked = self.locked();
                let targets: Vec<(String, &LiveServer)> = match server {
                    Some(name) => vec![
                        Self::resource_servers(&locked)
                            .into_iter()
                            .find(|(server, _)| server == &name)
                            .ok_or_else(|| failed(format!("unknown MCP server {name:?}")))?,
                    ],
                    None if cursor.is_some() => {
                        return Err(failed("a cursor needs its server".to_string()));
                    }
                    None => Self::resource_servers(&locked),
                };
                for (name, live) in &targets {
                    let (resources, cursor) = live
                        .resource_list(cursor.as_deref())
                        .map_err(lca_protocol::DispatchError::Failed)?;
                    for resource in resources {
                        rows.push(resource_row(name, &resource));
                    }
                    if targets.len() == 1 {
                        next = cursor.map(|page| format!("{name}:{page}"));
                    }
                }
                let mut answer = serde_json::json!({"resources": rows});
                if let Some(cursor) = next {
                    answer["nextCursor"] = cursor.into();
                }
                Ok(lca_protocol::ToolResult::ok(
                    call.call_id.clone(),
                    answer.to_string(),
                ))
            }
            crate::LIST_TEMPLATES_TOOL => {
                let mut rows = Vec::new();
                let locked = self.locked();
                let targets: Vec<(String, &LiveServer)> = match text("server") {
                    Some(name) => vec![
                        Self::resource_servers(&locked)
                            .into_iter()
                            .find(|(server, _)| server == &name)
                            .ok_or_else(|| failed(format!("unknown MCP server {name:?}")))?,
                    ],
                    None => Self::resource_servers(&locked),
                };
                for (name, live) in &targets {
                    let templates = live
                        .resource_templates()
                        .map_err(lca_protocol::DispatchError::Failed)?;
                    for template in templates {
                        rows.push(serde_json::json!({
                            "server": name,
                            "uri_template": template.uri_template,
                            "name": template.name,
                        }));
                    }
                }
                Ok(lca_protocol::ToolResult::ok(
                    call.call_id.clone(),
                    serde_json::json!({"templates": rows}).to_string(),
                ))
            }
            crate::READ_RESOURCE_TOOL => {
                let (Some(server), Some(uri)) = (text("server"), text("uri")) else {
                    return Err(failed(
                        "read_mcp_resource needs `server` and `uri`".to_string(),
                    ));
                };
                let locked = self.locked();
                let (_, live) = Self::resource_servers(&locked)
                    .into_iter()
                    .find(|(name, _)| name == &server)
                    .ok_or_else(|| failed(format!("unknown MCP server {server:?}")))?;
                let contents = live
                    .resource_read(&uri)
                    .map_err(lca_protocol::DispatchError::Failed)?;
                read_result(&call.call_id, &server, contents).map_err(failed)
            }
            _ => Err(failed(format!("unknown MCP tool {:?}", call.name))),
        }
    }

    /// Run one call against its server. A server-side `isError`
    /// becomes a tool error; a transport failure becomes a dispatch
    /// failure (the host records it as an extension error).
    pub async fn execute_tool(
        &self,
        call: &lca_protocol::ToolCall,
    ) -> Result<lca_protocol::ToolResult, lca_protocol::DispatchError> {
        if matches!(
            call.name.as_str(),
            crate::LIST_RESOURCES_TOOL | crate::LIST_TEMPLATES_TOOL | crate::READ_RESOURCE_TOOL
        ) {
            return self.execute_resource(call);
        }
        let servers = self.locked();
        let server = servers
            .iter()
            .filter_map(|server| server.live.as_ref())
            .find(|live| live.calls().contains_key(&call.name))
            .ok_or_else(|| {
                lca_protocol::DispatchError::Failed(format!("unknown MCP tool {:?}", call.name))
            })?;
        let name = server.calls()[&call.name].clone();
        let arguments: serde_json::Value = serde_json::from_str(&call.arguments)
            .map_err(|err| lca_protocol::DispatchError::Failed(format!("bad arguments: {err}")))?;
        let poisoned =
            || lca_protocol::DispatchError::Failed("MCP session lock is poisoned".to_string());
        let outcome = match server {
            LiveServer::Stdio { session, .. } => session
                .lock()
                .map_err(|_| poisoned())?
                .call(&name, &arguments)
                .map_err(lca_protocol::DispatchError::Failed)?,
            LiveServer::Http { session, .. } => session
                .lock()
                .map_err(|_| poisoned())?
                .call(&name, &arguments)
                .map_err(lca_protocol::DispatchError::Failed)?,
        };
        Ok(if outcome.error {
            lca_protocol::ToolResult::error(call.call_id.clone(), outcome.text)
        } else {
            lca_protocol::ToolResult::ok(call.call_id.clone(), outcome.text)
        })
    }
}

impl Drop for McpBridge {
    /// Stopping a server closes its pipes and kills its tree (pi
    /// closes stdin, then SIGTERM, then SIGKILL; the host's
    /// `process_kill` owns that sequence).
    fn drop(&mut self) {
        for server in self
            .servers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter_mut()
        {
            server.shutdown();
        }
    }
}

impl lca_ext_abi::ExtensionDispatch for McpBridge {
    fn name(&self) -> &str {
        "mcp"
    }

    fn delivery(&self) -> lca_ext_abi::DeliveryMode {
        lca_ext_abi::DeliveryMode::Native
    }

    fn worlds(&self) -> Vec<lca_ext_abi::World> {
        vec![lca_ext_abi::World::ToolCatalog]
    }

    fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, lca_protocol::DispatchError> {
        McpBridge::tool_specs(self)
    }

    fn execute_tool<'a>(
        &'a self,
        call: &'a lca_protocol::ToolCall,
    ) -> lca_ext_abi::DispatchFuture<
        'a,
        Result<lca_protocol::ToolResult, lca_protocol::DispatchError>,
    > {
        Box::pin(McpBridge::execute_tool(self, call))
    }

    fn command_specs(&self) -> Result<Vec<lca_protocol::CommandSpec>, lca_protocol::DispatchError> {
        Err(lca_protocol::DispatchError::MissingWorld {
            extension: self.name().to_string(),
            world: "command",
        })
    }

    fn invoke_command(
        &self,
        _name: &str,
        _argument: &str,
    ) -> Result<lca_protocol::CommandEffect, lca_protocol::DispatchError> {
        Err(lca_protocol::DispatchError::MissingWorld {
            extension: self.name().to_string(),
            world: "command",
        })
    }

    // The bridge observes nothing: it contributes tools, not policy.
    fn on_pre_turn(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_pre_tool_use<'a>(
        &'a self,
        _call: &'a lca_protocol::ToolCall,
    ) -> lca_ext_abi::DispatchFuture<
        'a,
        Result<lca_protocol::HookAction, lca_protocol::DispatchError>,
    > {
        Box::pin(std::future::ready(Ok(lca_protocol::HookAction::Allow)))
    }

    fn on_post_tool_use<'a>(
        &'a self,
        _observation: &'a lca_protocol::PostToolObservation,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_post_turn_end<'a>(
        &'a self,
        _status: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_attention_required<'a>(
        &'a self,
        _reason: &'a str,
    ) -> lca_ext_abi::DispatchFuture<'a, Result<(), lca_protocol::DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn on_session_close(
        &self,
    ) -> lca_ext_abi::DispatchFuture<'static, Result<(), lca_protocol::DispatchError>> {
        Box::pin(std::future::ready(Ok(())))
    }
}
