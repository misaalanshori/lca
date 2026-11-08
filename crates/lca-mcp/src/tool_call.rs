//! One MCP tool call, with the server table unlocked and the blocking work off
//! the runtime thread (gh #554).
//!
//! Split out of `native.rs` when the resource trio's two-stage lookup pushed it
//! toward gate 11's 1,200-line ceiling. The rule this file exists to enforce:
//!
//! * **The table is a lookup, not a lease.** `execute_tool` clones what the call
//!   needs - the server-side tool name and a [`SessionHandle`] - and drops the
//!   `servers` guard *before* any I/O. A 60 s `tools/call` used to hold that
//!   guard for its whole duration, so `/mcp`, the status line, a settings change
//!   and `process_kill` all waited behind it. `process_kill` is what cancellation
//!   calls, so a slow server could freeze a turn's cancel.
//! * **The session lock is not the table lock.** [`SessionHandle`] clones the
//!   row's `Arc<Mutex<Session<..>>>`, so one JSON-RPC exchange at a time per
//!   server still holds (response ids stay matched to their requests) without
//!   the table being held at all. That `Arc` is the same session the row owns -
//!   not a second registry.
//! * **Blocking work runs on `spawn_blocking`.** The pipe write/read and the HTTP
//!   exchange are synchronous by design (both transports bound themselves), and
//!   a turn awaits this from a Tokio worker. Embedders may run a current-thread
//!   runtime, where `block_in_place` panics, so `spawn_blocking` is the only
//!   answer that is correct in both.
//!
//! A server removed from the table mid-call (a concurrent edit, `land`, or
//! `shutdown`) is fine: the cloned `Arc` keeps the session alive for the
//! exchange's duration and the answer goes to the caller who asked for it. The
//! clone is why that is true rather than accidental.

use std::sync::{Arc, Mutex};

use crate::native::{LiveServer, McpBridge};

/// One server's session, held **without** the server table (gh #554).
///
/// The table owns the *set* of servers and hands out this handle for the
/// duration of one exchange. The `Arc` is not a second registry: it is the very
/// session the row owns, reachable after the row's lock is gone. That is what
/// lets a long call run with the table unlocked.
pub(crate) enum SessionHandle {
    Stdio(Arc<Mutex<crate::Session<crate::native::ChildPipe>>>),
    Http(Arc<Mutex<crate::remote::HttpSession>>),
}

impl SessionHandle {
    fn poisoned() -> String {
        "MCP session lock is poisoned".to_string()
    }

    /// One `tools/call`, with only this server's session locked.
    fn call(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<crate::ToolOutcome, String> {
        match self {
            SessionHandle::Stdio(session) => session
                .lock()
                .map_err(|_| Self::poisoned())?
                .call(name, arguments),
            SessionHandle::Http(session) => session
                .lock()
                .map_err(|_| Self::poisoned())?
                .call(name, arguments),
        }
    }

    fn resource_list(
        &self,
        cursor: Option<&str>,
    ) -> Result<(Vec<crate::McpResource>, Option<String>), String> {
        match self {
            SessionHandle::Stdio(session) => session
                .lock()
                .map_err(|_| Self::poisoned())?
                .resource_list(cursor),
            SessionHandle::Http(session) => session
                .lock()
                .map_err(|_| Self::poisoned())?
                .resource_list(cursor),
        }
    }

    fn resource_templates(&self) -> Result<Vec<crate::McpResourceTemplate>, String> {
        match self {
            SessionHandle::Stdio(session) => session
                .lock()
                .map_err(|_| Self::poisoned())?
                .resource_templates(),
            SessionHandle::Http(session) => session
                .lock()
                .map_err(|_| Self::poisoned())?
                .resource_templates(),
        }
    }

    fn resource_read(&self, uri: &str) -> Result<Vec<crate::ResourceContent>, String> {
        match self {
            SessionHandle::Stdio(session) => session
                .lock()
                .map_err(|_| Self::poisoned())?
                .resource_read(uri),
            SessionHandle::Http(session) => session
                .lock()
                .map_err(|_| Self::poisoned())?
                .resource_read(uri),
        }
    }
}

impl LiveServer {
    /// The session behind this server, cloned so the caller can hold it with
    /// the server table unlocked (gh #554). Cheap: an `Arc` clone, and the
    /// per-server lock inside still serialises one exchange at a time.
    #[allow(clippy::wrong_self_convention)] // it returns a handle, not a session
    pub(crate) fn session(&self) -> SessionHandle {
        match self {
            LiveServer::Stdio { session, .. } => SessionHandle::Stdio(Arc::clone(session)),
            LiveServer::Http { session, .. } => SessionHandle::Http(Arc::clone(session)),
        }
    }
}

/// One server offering resources, resolved under the table lock and used after
/// it: the name for the reply, the handle for the I/O.
pub(crate) struct ResourceTarget {
    pub(crate) name: String,
    pub(crate) session: SessionHandle,
}

impl McpBridge {
    /// Run one call against its server. A server-side `isError` becomes a tool
    /// error; a transport failure becomes a dispatch failure (the host records
    /// it as an extension error).
    ///
    /// **The table is looked at, never held.** The guard is dropped before the
    /// blocking exchange, which is the whole of gh #554's first half.
    pub async fn execute_tool(
        &self,
        call: &lca_protocol::ToolCall,
    ) -> Result<lca_protocol::ToolResult, lca_protocol::DispatchError> {
        if matches!(
            call.name.as_str(),
            crate::LIST_RESOURCES_TOOL | crate::LIST_TEMPLATES_TOOL | crate::READ_RESOURCE_TOOL
        ) {
            return self.execute_resource(call).await;
        }
        // Step 1: a lookup, under the lock, that ends when the guard drops.
        let (name, session) = {
            let servers = self.locked();
            let server = servers
                .iter()
                .filter_map(|server| server.live.as_ref())
                .find(|live| live.calls().contains_key(&call.name))
                .ok_or_else(|| {
                    lca_protocol::DispatchError::Failed(format!("unknown MCP tool {:?}", call.name))
                })?;
            (server.calls()[&call.name].clone(), server.session())
        };
        let arguments: serde_json::Value = serde_json::from_str(&call.arguments)
            .map_err(|err| lca_protocol::DispatchError::Failed(format!("bad arguments: {err}")))?;
        // Step 2: the exchange, with no table lock at all (step 3 is the
        // blocking pool: a pipe/HTTP round trip is synchronous by design).
        let outcome = run_blocking("tools/call", move || session.call(&name, &arguments)).await?;
        Ok(if outcome.error {
            lca_protocol::ToolResult::error(call.call_id.clone(), outcome.text)
        } else {
            lca_protocol::ToolResult::ok(call.call_id.clone(), outcome.text)
        })
    }

    /// Run one resource call (the trio routes here before the server table:
    /// the tools belong to the bridge, not to one server).
    ///
    /// Same two-stage shape as [`McpBridge::execute_tool`]: resolve the targets
    /// under the lock, then do every exchange with the table released. The
    /// staging write at the end is blocking too, so the whole arm runs on the
    /// blocking pool.
    async fn execute_resource(
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
        let name = call.name.clone();
        match name.as_str() {
            crate::LIST_RESOURCES_TOOL => {
                let (server, cursor) = match (text("server"), text("cursor")) {
                    (_, Some(cursor)) => cursor
                        .split_once(':')
                        .map(|(server, page)| (Some(server.to_string()), Some(page.to_string())))
                        .ok_or_else(|| failed("bad cursor".to_string()))?,
                    (server, None) => (server, None),
                };
                let targets = {
                    let locked = self.locked();
                    match server {
                        Some(server) => vec![Self::resource_target(&locked, &server)?],
                        None if cursor.is_some() => {
                            return Err(failed("a cursor needs its server".to_string()));
                        }
                        None => Self::resource_targets(&locked),
                    }
                };
                let call_id = call.call_id.clone();
                let rows = run_blocking("resources/list", move || {
                    let mut rows = Vec::new();
                    let mut next = None;
                    for target in &targets {
                        let (resources, page) = target
                            .session
                            .resource_list(cursor.as_deref())
                            .map_err(|err| format!("MCP server {:?}: {err}", target.name))?;
                        for resource in resources {
                            rows.push(crate::native::resource_row(&target.name, &resource));
                        }
                        if targets.len() == 1 {
                            next = page.map(|page| format!("{}:{page}", target.name));
                        }
                    }
                    let mut answer = serde_json::json!({ "resources": rows });
                    if let Some(cursor) = next {
                        answer["nextCursor"] = cursor.into();
                    }
                    Ok::<_, String>((answer, call_id))
                })
                .await?;
                let (answer, call_id) = rows;
                Ok(lca_protocol::ToolResult::ok(call_id, answer.to_string()))
            }
            crate::LIST_TEMPLATES_TOOL => {
                let targets = {
                    let locked = self.locked();
                    match text("server") {
                        Some(server) => vec![Self::resource_target(&locked, &server)?],
                        None => Self::resource_targets(&locked),
                    }
                };
                let call_id = call.call_id.clone();
                let rows = run_blocking("resources/templates/list", move || {
                    let mut rows = Vec::new();
                    for target in &targets {
                        let templates = target
                            .session
                            .resource_templates()
                            .map_err(|err| format!("MCP server {:?}: {err}", target.name))?;
                        for template in templates {
                            rows.push(serde_json::json!({
                                "server": target.name,
                                "uri_template": template.uri_template,
                                "name": template.name,
                            }));
                        }
                    }
                    Ok::<_, String>((rows, call_id))
                })
                .await?;
                let (rows, call_id) = rows;
                Ok(lca_protocol::ToolResult::ok(
                    call_id,
                    serde_json::json!({ "templates": rows }).to_string(),
                ))
            }
            crate::READ_RESOURCE_TOOL => {
                let (Some(server), Some(uri)) = (text("server"), text("uri")) else {
                    return Err(failed(
                        "read_mcp_resource needs `server` and `uri`".to_string(),
                    ));
                };
                let target = {
                    let locked = self.locked();
                    Self::resource_target(&locked, &server)?
                };
                let call_id = call.call_id.clone();
                let caps = self.caps.clone();
                let result = run_blocking("resources/read", move || {
                    let contents = target
                        .session
                        .resource_read(&uri)
                        .map_err(|err| format!("MCP server {:?}: {err}", target.name))?;
                    // Staging a binary payload writes a file through the temp
                    // scope, so this is blocking work too - and it must not run
                    // with the table held, which it no longer is.
                    crate::native::read_result(&caps, &call_id, &server, contents)
                })
                .await?;
                Ok(result)
            }
            other => Err(failed(format!("unknown MCP tool {other:?}"))),
        }
    }

    /// Every server offering resources, resolved under the table lock.
    fn resource_targets(servers: &[crate::native::ManagedServer]) -> Vec<ResourceTarget> {
        servers
            .iter()
            .filter_map(|server| {
                let live = server.live.as_ref()?;
                (live.offers_resources() && live.exposure() != lca_protocol::ToolExposure::Hidden)
                    .then(|| ResourceTarget {
                        name: server.entry.name.clone(),
                        session: live.session(),
                    })
            })
            .collect()
    }

    /// One named server offering resources.
    fn resource_target(
        servers: &[crate::native::ManagedServer],
        name: &str,
    ) -> Result<ResourceTarget, lca_protocol::DispatchError> {
        Self::resource_targets(servers)
            .into_iter()
            .find(|target| target.name == name)
            .ok_or_else(|| {
                lca_protocol::DispatchError::Failed(format!("unknown MCP server {name:?}"))
            })
    }
}

/// Run blocking MCP work on the blocking pool and map a panic in it to a named
/// dispatch failure (gh #554).
///
/// Not `block_in_place`: the turn is awaited from a Tokio worker, and an
/// embedder may run a current-thread runtime where `block_in_place` panics.
/// `spawn_blocking` is correct under both, which is why it is the only shape
/// here.
async fn run_blocking<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, lca_protocol::DispatchError> {
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(detail)) => Err(lca_protocol::DispatchError::Failed(detail)),
        Err(join) => Err(lca_protocol::DispatchError::Failed(format!(
            "MCP {what} failed: {}",
            if join.is_panic() {
                "the worker panicked"
            } else {
                "the worker did not finish"
            }
        ))),
    }
}
