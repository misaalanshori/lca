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
    },
    Http {
        session: Mutex<crate::remote::HttpSession>,
        specs: Vec<lca_protocol::ToolSpec>,
        /// Qualified name back to the server-side tool name.
        calls: BTreeMap<String, String>,
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
/// state row for every managed entry (connected or not).
pub struct McpBridge {
    servers: Vec<ManagedServer>,
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
        session.initialize()?;
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
    if let Err(err) = session.initialize() {
        return Err(if err.contains(crate::remote::NEEDS_AUTH_MARKER) {
            ServerStateKind::NeedsSignIn(err)
        } else {
            ServerStateKind::Failed(err)
        });
    }
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
        McpBridge { servers }
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
        for server in &bridge.servers {
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
        for server in &bridge.servers {
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
    pub fn statuses(&self) -> Vec<ServerStatus> {
        self.servers.iter().map(ManagedServer::status).collect()
    }

    /// Every bridged tool, sorted (the registry's stable order).
    pub fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, lca_protocol::DispatchError> {
        let mut specs = Vec::new();
        for server in &self.servers {
            if let Some(live) = &server.live {
                specs.extend(live.specs().iter().cloned());
            }
        }
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(specs)
    }

    /// Run one call against its server. A server-side `isError`
    /// becomes a tool error; a transport failure becomes a dispatch
    /// failure (the host records it as an extension error).
    pub async fn execute_tool(
        &self,
        call: &lca_protocol::ToolCall,
    ) -> Result<lca_protocol::ToolResult, lca_protocol::DispatchError> {
        let server = self
            .servers
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
        for server in &mut self.servers {
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
