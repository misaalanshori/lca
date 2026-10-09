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

/// One live server: its session (behind a lock - one JSON-RPC exchange
/// at a time per server, which is also what keeps response ids matched
/// to their requests) plus the specs listed at connect.
struct LiveServer {
    caps: Arc<lca_tools::Capabilities>,
    handle: u32,
    session: Mutex<crate::Session<ChildPipe>>,
    specs: Vec<lca_protocol::ToolSpec>,
    /// Qualified name back to the server-side tool name.
    calls: BTreeMap<String, String>,
}

impl LiveServer {
    fn shutdown(&self) {
        let _ = self.caps.process_kill(self.handle);
    }
}

/// The bridge: one long-lived child per configured server.
pub struct McpBridge {
    servers: Vec<LiveServer>,
}

impl McpBridge {
    /// Spawn every server (each spawn asks the permission layer),
    /// handshake, and list tools. A refusal stops the bridge: a server
    /// the user declined never starts, and the denial is recorded on
    /// the engine by the spawn itself.
    pub fn connect(
        caps: Arc<lca_tools::Capabilities>,
        servers: Vec<ServerConfig>,
    ) -> Result<McpBridge, String> {
        let mut live = Vec::with_capacity(servers.len());
        for server in &servers {
            let handle = caps
                .process_spawn(&server.command, &server.args, &server.cwd_scope)
                .map_err(|err| format!("MCP server {:?} did not start: {err}", server.name))?;
            let outcome = (|| -> Result<LiveServer, String> {
                let mut session = crate::Session::new(ChildPipe {
                    caps: caps.clone(),
                    handle,
                });
                session.initialize()?;
                let listed = session.tools(&server.name)?;
                let calls = listed
                    .iter()
                    .map(|tool| (tool.qualified.clone(), tool.name.clone()))
                    .collect();
                let specs = qualify_tools(&server.name, listed)?;
                Ok(LiveServer {
                    caps: caps.clone(),
                    handle,
                    session: Mutex::new(session),
                    specs,
                    calls,
                })
            })();
            match outcome {
                Ok(server) => live.push(server),
                Err(err) => {
                    let _ = caps.process_kill(handle);
                    for server in &live {
                        server.shutdown();
                    }
                    return Err(format!("MCP server {:?} failed: {err}", server.name));
                }
            }
        }
        Ok(McpBridge { servers: live })
    }

    /// Every bridged tool, sorted (the registry's stable order).
    pub fn tool_specs(&self) -> Result<Vec<lca_protocol::ToolSpec>, lca_protocol::DispatchError> {
        let mut specs = Vec::new();
        for server in &self.servers {
            specs.extend(server.specs.iter().cloned());
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
            .find(|server| server.calls.contains_key(&call.name))
            .ok_or_else(|| {
                lca_protocol::DispatchError::Failed(format!("unknown MCP tool {:?}", call.name))
            })?;
        let name = server.calls[&call.name].clone();
        let arguments: serde_json::Value = serde_json::from_str(&call.arguments)
            .map_err(|err| lca_protocol::DispatchError::Failed(format!("bad arguments: {err}")))?;
        let outcome = server
            .session
            .lock()
            .map_err(|_| {
                lca_protocol::DispatchError::Failed("MCP session lock is poisoned".to_string())
            })?
            .call(&name, &arguments)
            .map_err(lca_protocol::DispatchError::Failed)?;
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
        for server in &self.servers {
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
