//! The sandboxed twin: the `tool-catalog` component behind the same
//! protocol core, speaking through the `process` and `state` host
//! imports. The server list arrives as JSON in [`SERVERS_STATE_KEY`](crate::SERVERS_STATE_KEY)
//! (seeding it from the manifest is the phase-2 management cut line);
//! sessions stay live across calls in a static, reconnecting when the
//! configured list changes.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::{ServerConfig, Session, Stdio, parse_servers, qualify_tools};

wit_bindgen::generate!({
    path: "../../wit",
    world: "tool-catalog",
    export_macro_name: "export_mcp_catalog",
    with: {
        "lca:host/log@0.6.0": generate,
        "lca:host/fs@0.6.0": generate,
        "lca:host/process@0.6.0": generate,
        "lca:host/pty@0.6.0": generate,
        "lca:host/ui-dialogs@0.6.0": generate,
        "lca:host/resources@0.6.0": generate,
        "lca:host/state@0.6.0": generate,
        "lca:host/tools@0.6.0": generate,
    },
});

use exports::lca::ext::catalog::{Guest as CatalogGuest, ToolAnnotations, ToolNamespace, ToolSpec};
use exports::lca::ext::catalog_run::{Guest as RunGuest, ToolResult as WasmResult};
use lca::ext::types::{ExtraPair, ToolCall};

fn process_error(err: lca::host::process::Error) -> String {
    use lca::host::process::Error as E;
    match err {
        E::Permission(detail) => format!("the user declined the MCP server: {detail}"),
        E::NotGranted(detail) => format!("the manifest does not grant process: {detail}"),
        E::NotFound(detail) => format!("unknown MCP server handle: {detail}"),
        E::Io(detail) => format!("MCP server pipe failed: {detail}"),
        E::Invalid(detail) => format!("bad MCP server call: {detail}"),
    }
}

/// [`Stdio`](crate::Stdio) over one child the host spawned.
struct GuestPipe {
    handle: u32,
}

impl Stdio for GuestPipe {
    fn write_all(&self, bytes: &[u8]) -> Result<(), String> {
        lca::host::process::write_stdin(self.handle, bytes)
            .map(|_| ())
            .map_err(process_error)
    }

    fn read_some(&self, max: usize) -> Result<Option<Vec<u8>>, String> {
        lca::host::process::read_stdout(self.handle, max as u64).map_err(process_error)
    }
}

/// One live server behind the component's static.
struct GuestServer {
    handle: u32,
    session: Session<GuestPipe>,
    /// Qualified name back to the server-side tool name.
    calls: BTreeMap<String, String>,
    specs: Vec<ToolSpec>,
}

fn to_wit_spec(spec: &lca_protocol::ToolSpec) -> ToolSpec {
    ToolSpec {
        name: spec.name.clone(),
        description: spec.description.clone(),
        parameters: spec.parameters.to_string(),
        exposure: spec.exposure.as_str().to_string(),
        namespace: spec.namespace.as_ref().map(|namespace| ToolNamespace {
            name: namespace.name.clone(),
            description: namespace.description.clone(),
            instructions: namespace.instructions.clone(),
        }),
        annotations: spec
            .annotations
            .as_ref()
            .map(|annotations| ToolAnnotations {
                read_only_hint: annotations.read_only_hint,
                destructive_hint: annotations.destructive_hint,
                idempotent_hint: annotations.idempotent_hint,
                open_world_hint: annotations.open_world_hint,
            }),
        extras: spec
            .extras
            .iter()
            .map(|(key, value)| ExtraPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

impl GuestServer {
    fn connect(config: &ServerConfig) -> Result<GuestServer, String> {
        let handle = lca::host::process::spawn(&config.command, &config.args, &config.cwd_scope)
            .map_err(process_error)
            .map_err(|err| format!("MCP server {:?} did not start: {err}", config.name))?;
        let mut session = Session::new(GuestPipe { handle });
        let outcome = (|| -> Result<(BTreeMap<String, String>, Vec<ToolSpec>), String> {
            session.initialize()?;
            let listed = session.tools(&config.name)?;
            let calls = listed
                .iter()
                .map(|tool| (tool.qualified.clone(), tool.name.clone()))
                .collect();
            let specs = qualify_tools(&config.name, listed)?
                .iter()
                .map(to_wit_spec)
                .collect();
            Ok((calls, specs))
        })();
        match outcome {
            Ok((calls, specs)) => Ok(GuestServer {
                handle,
                session,
                calls,
                specs,
            }),
            Err(err) => {
                let _ = lca::host::process::kill(handle);
                Err(format!("MCP server {:?} failed: {err}", config.name))
            }
        }
    }

    fn shutdown(&self) {
        let _ = lca::host::process::kill(self.handle);
    }
}

/// The live bridge, rebuilt when the configured servers change.
struct Bridge {
    fingerprint: String,
    servers: Vec<GuestServer>,
}

static BRIDGE: OnceLock<Mutex<Option<Bridge>>> = OnceLock::new();

/// Connect (or reuse) the bridge for the currently configured servers.
fn bridge() -> Result<(), String> {
    let configs = match lca::host::state::read(crate::SERVERS_STATE_KEY) {
        Some(bytes) => {
            let json = String::from_utf8(bytes)
                .map_err(|_| "the MCP server list is not UTF-8".to_string())?;
            parse_servers(&json)?
        }
        None => Vec::new(),
    };
    let fingerprint = serde_json::to_string(&configs)
        .map_err(|err| format!("cannot fingerprint the MCP server list: {err}"))?;
    let cell = BRIDGE.get_or_init(|| Mutex::new(None));
    let mut slot = cell
        .lock()
        .map_err(|_| "the MCP bridge lock is poisoned".to_string())?;
    let stale = slot
        .as_ref()
        .is_none_or(|bridge| bridge.fingerprint != fingerprint);
    if !stale {
        return Ok(());
    }
    if let Some(bridge) = slot.take() {
        for server in &bridge.servers {
            server.shutdown();
        }
    }
    let mut servers = Vec::with_capacity(configs.len());
    for config in &configs {
        match GuestServer::connect(config) {
            Ok(server) => servers.push(server),
            Err(err) => {
                for server in &servers {
                    server.shutdown();
                }
                return Err(err);
            }
        }
    }
    *slot = Some(Bridge {
        fingerprint,
        servers,
    });
    Ok(())
}

fn with_bridge<T>(run: impl FnOnce(&mut Bridge) -> Result<T, String>) -> Result<T, String> {
    bridge()?;
    let cell = BRIDGE.get().expect("bridge connects before it runs");
    let mut slot = cell
        .lock()
        .map_err(|_| "the MCP bridge lock is poisoned".to_string())?;
    let Some(bridge) = slot.as_mut() else {
        return Err("the MCP bridge is not connected".to_string());
    };
    run(bridge)
}

pub struct McpCatalog;

impl CatalogGuest for McpCatalog {
    fn get_tools() -> Vec<ToolSpec> {
        with_bridge(|bridge| {
            let mut specs = Vec::new();
            for server in &bridge.servers {
                specs.extend(server.specs.iter().cloned());
            }
            specs.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(specs)
        })
        .unwrap_or_default()
    }
}

impl RunGuest for McpCatalog {
    fn run_tool(name: String, call: ToolCall) -> WasmResult {
        let fail = |detail: String| WasmResult {
            call_id: call.call_id.clone(),
            status: "error".to_string(),
            content: Some(detail),
            truncated: false,
            extras: Vec::new(),
        };
        let outcome = with_bridge(|bridge| {
            let server = bridge
                .servers
                .iter_mut()
                .find(|server| server.calls.contains_key(&name))
                .ok_or_else(|| format!("unknown MCP tool {name:?}"))?;
            let tool = server.calls[&name].clone();
            let arguments: serde_json::Value = serde_json::from_str(&call.arguments)
                .map_err(|err| format!("bad arguments: {err}"))?;
            server.session.call(&tool, &arguments)
        });
        match outcome {
            Ok(answer) => WasmResult {
                call_id: call.call_id,
                status: if answer.error { "error" } else { "ok" }.to_string(),
                content: Some(answer.text),
                truncated: false,
                extras: Vec::new(),
            },
            Err(detail) => fail(detail),
        }
    }
}

export_mcp_catalog!(McpCatalog);
