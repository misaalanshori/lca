//! MCP bridge (gh #53 phase 1): external MCP servers over stdio as one
//! tool-catalog suite.
//!
//! The shape is ADR-0045's extension bridge, not a host service: the
//! extension spawns each configured server through the `process`
//! capability (one long-lived child per server, newline-delimited
//! JSON-RPC over its stdio), names its tools pi's
//! `mcp__<server>__<tool>`, and serves them `direct`. Spawning passes
//! the same permission prompt as a model-requested command, so a
//! declined server never starts and the denial is recorded; per-call
//! arguments flow to an already-approved server, and every call runs
//! through the turn's `tool_call`/`tool_result` hooks like any other
//! extension tool.
//!
//! Layout: this module is the shared protocol core (both targets).
//! `native` is the in-process handle the scripted turn drives;
//! `guest` is the sandboxed component twin behind the `tool-catalog`
//! world. Later phases (OAuth/remote/resources/management) extend this
//! core; they do not replace it.

pub mod config;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
#[cfg(not(target_arch = "wasm32"))]
pub mod remote;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{McpBridge, manifest_grants};
#[cfg(not(target_arch = "wasm32"))]
pub use remote::{TokenStore, refresh, sign_in};
#[cfg(target_arch = "wasm32")]
mod guest;

use std::collections::BTreeMap;

/// The `state` key the sandboxed guest reads its server list from (a
/// JSON array of [`ServerConfig`]). Seeding it from the manifest is the
/// phase-2 management cut line; phase 1 seeds it by hand.
pub const SERVERS_STATE_KEY: &str = "mcp-servers";

/// One stdio server to bridge (pi's `mcpServers` entry, stdio subset).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServerConfig {
    /// pi's server name: letters, digits, `_`, `-`.
    pub name: String,
    /// One executable (argv, never a shell string).
    pub command: String,
    /// Its arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// The `fs` scope the server runs in (pi's `cwd`).
    pub cwd_scope: String,
}

/// One remote server to bridge (pi's `mcpServers` entry, HTTP subset).
/// Additive: the stdio [`ServerConfig`] above is untouched.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HttpServerConfig {
    /// pi's server name: letters, digits, `_`, `-`.
    pub name: String,
    /// The streamable-HTTP endpoint (pi's `url`).
    pub url: String,
    /// Extra headers (pi's `headers`); an `Authorization` header
    /// disables OAuth for the server, exactly like pi.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Per-request timeout in seconds (pi's `timeout`, default 60).
    /// Zero means the default.
    #[serde(default)]
    pub timeout_secs: u64,
    /// OAuth settings (`None` means no sign-in flow configured).
    #[serde(default)]
    pub oauth: Option<OAuthConfig>,
}

/// OAuth settings for one remote server (pi's `oauth` object, the
/// phase-2 subset: dynamic registration plus scope).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OAuthConfig {
    /// `client_name` for dynamic registration (default `lca`).
    #[serde(default)]
    pub client_name: Option<String>,
    /// Space-separated scopes to request.
    #[serde(default)]
    pub scope: Option<String>,
    /// The authorization server's metadata document: skips discovery
    /// (pi's `authServerMetadataUrl`).
    #[serde(default)]
    pub auth_server_metadata_url: Option<String>,
    /// A pre-registered client (pi's `clientId`): skips dynamic
    /// registration.
    #[serde(default)]
    pub client_id: Option<String>,
    /// Its secret, when the server issued one.
    #[serde(default)]
    pub client_secret: Option<String>,
}

impl HttpServerConfig {
    /// The effective per-request timeout (pi's 60-second default).
    pub fn timeout(&self) -> std::time::Duration {
        let secs = if self.timeout_secs == 0 {
            60
        } else {
            self.timeout_secs.min(600)
        };
        std::time::Duration::from_secs(secs)
    }

    /// Whether the configured headers already authenticate (pi:
    /// OAuth applies only without an `Authorization` header).
    pub fn has_static_auth(&self) -> bool {
        self.headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("authorization"))
    }
}

/// One stored OAuth credential set (`mcp-auth.json`-shaped, keyed by
/// server name and URL like pi: servers sharing a URL keep separate
/// accounts).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredToken {
    /// The bearer token.
    pub access_token: String,
    /// For proactive refresh and the 401 retry.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Milliseconds since the Unix epoch, when the access token dies.
    #[serde(default)]
    pub expires_at_ms: Option<u64>,
    /// The dynamically registered client (or configured) identity.
    #[serde(default)]
    pub client_id: String,
    /// Issued at registration, when the server sends one.
    #[serde(default)]
    pub client_secret: Option<String>,
    /// The granted scopes, space-separated.
    #[serde(default)]
    pub scope: Option<String>,
    /// Where the tokens came from (refresh posts here).
    #[serde(default)]
    pub token_url: String,
}

/// The credentials key for one server's tokens.
pub fn token_key(name: &str, url: &str) -> String {
    format!("mcp-oauth:{name}:{url}")
}

/// Scopes of every list, each once (pi's `mergeScopes`): step-up
/// adds to the granted scopes, never replaces them.
pub fn merge_scopes(scopes: &[Option<&str>]) -> Option<String> {
    let mut merged = Vec::new();
    for scope in scopes.iter().flatten() {
        for part in scope.split_whitespace() {
            if !merged.contains(&part) {
                merged.push(part);
            }
        }
    }
    (!merged.is_empty()).then(|| merged.join(" "))
}

/// Parse a `WWW-Authenticate: Bearer ...` challenge into its
/// parameters (quoted-string aware; unquoted tokens kept bare).
pub fn parse_www_authenticate(header: &str) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    let challenge = header.trim();
    let rest = challenge
        .strip_prefix("Bearer")
        .or_else(|| challenge.strip_prefix("bearer"))
        .unwrap_or(challenge);
    let mut current = String::new();
    let mut key: Option<String> = None;
    let mut quoted = false;
    let mut parts = Vec::new();
    for char in rest.chars() {
        match char {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(char),
        }
    }
    parts.push(current);
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match (part.split_once('='), key.take()) {
            (Some((name, value)), _) => {
                params.insert(name.trim().to_string(), value.trim().to_string());
            }
            (None, _) => {
                key = Some(part.to_string());
            }
        }
    }
    let _ = key;
    params
}

/// Pi's transient set: 408, 429, and 5xx except 501 are worth another
/// attempt. The bridge retries the idempotent reads (`initialize`,
/// `tools/list`); a `tools/call` reports the first failure instead
/// (the server may already have performed it, so retrying could run
/// it twice).
pub fn is_transient_status(status: u16) -> bool {
    status == 408 || status == 429 || (status >= 500 && status != 501)
}

/// Parse and validate a server list (the guest's `state` read and any
/// future config loader share this, so both refuse the same entries).
pub fn parse_servers(json: &str) -> Result<Vec<ServerConfig>, String> {
    let servers: Vec<ServerConfig> =
        serde_json::from_str(json).map_err(|err| format!("invalid MCP server list: {err}"))?;
    for server in &servers {
        if !is_valid_server_name(&server.name) {
            return Err(format!(
                "invalid MCP server name {:?}: letters, digits, `_`, `-` only",
                server.name
            ));
        }
        if server.command.is_empty() {
            return Err(format!("MCP server {:?} has no command", server.name));
        }
    }
    Ok(servers)
}

/// pi's server-name rule (`docs/mcp.md`: letters, digits, `_`, `-`).
pub fn is_valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// pi's spelling rule: every character outside letters, digits, and
/// `_` becomes `_` (tool names and namespace segments alike).
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// pi's tool name: `mcp__<server>__<tool>`.
pub fn tool_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", sanitize(server), sanitize(tool))
}

/// One tool the server listed, before namespacing.
#[derive(Debug, Clone)]
pub struct ServerTool {
    /// The name as the server listed it.
    pub name: String,
    /// The namespaced name the model calls.
    pub qualified: String,
    /// What the model sees.
    pub description: String,
    /// The arguments schema.
    pub parameters: serde_json::Value,
    /// The server's annotations (model-visible, never deciding).
    pub annotations: lca_protocol::ToolAnnotations,
}

/// Qualify one server's tools: pi names, per-tool exposure from the
/// server default plus its `toolExposure` rules, one namespace per
/// server. `hidden` tools are excluded (registered but unreachable).
/// A sanitized collision refuses the server (pi's hash suffix is a
/// later-phase nicety, ADR-0045).
pub fn qualify_tools(
    server: &str,
    tools: Vec<ServerTool>,
    default: config::McpExposure,
    rules: &[(String, config::McpExposure)],
) -> Result<Vec<lca_protocol::ToolSpec>, String> {
    let mut seen = BTreeMap::new();
    let mut specs = Vec::with_capacity(tools.len());
    for tool in tools {
        if let Some(first) = seen.insert(tool.qualified.clone(), tool.name.clone()) {
            return Err(format!(
                "MCP server {server:?} lists tools {first:?} and {:?} under one name {:?}",
                tool.name, tool.qualified
            ));
        }
        let exposure = config::exposure_for(default, rules, &tool.name);
        if exposure == config::McpExposure::Hidden {
            continue;
        }
        specs.push(lca_protocol::ToolSpec {
            name: tool.qualified.clone(),
            description: tool.description,
            parameters: tool.parameters,
            exposure: exposure.as_tool_exposure(),
            namespace: Some(lca_protocol::ToolNamespace {
                name: format!("mcp__{}", sanitize(server)),
                description: format!("Tools from MCP server `{server}`."),
                instructions: None,
            }),
            annotations: Some(tool.annotations),
            extras: BTreeMap::new(),
        });
    }
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(specs)
}

/// What a `tools/call` returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    /// The server set `isError`.
    pub error: bool,
    /// The concatenated text items.
    pub text: String,
}

/// The byte pipe both adapters speak: blocking writes, blocking reads
/// of whatever is available (`None` at EOF).
pub trait Stdio: Send {
    /// Write every byte (flush included).
    fn write_all(&self, bytes: &[u8]) -> Result<(), String>;
    /// Read up to `max` bytes; `None` at EOF.
    fn read_some(&self, max: usize) -> Result<Option<Vec<u8>>, String>;
}

/// Read one `\n`-terminated line, accumulating into `buf`. Bounded: a
/// server that never terminates a line refuses the call instead of
/// hanging the turn.
pub fn read_line(io: &dyn Stdio, buf: &mut Vec<u8>) -> Result<String, String> {
    loop {
        if let Some(end) = buf.iter().position(|byte| *byte == b'\n') {
            let line = buf.drain(..=end).collect::<Vec<u8>>();
            return String::from_utf8(line)
                .map_err(|_| "MCP server answered non-UTF-8".to_string());
        }
        if buf.len() > 1024 * 1024 {
            return Err("MCP server answered over 1 MiB without a newline".to_string());
        }
        match io.read_some(64 * 1024) {
            Ok(Some(chunk)) => buf.extend_from_slice(&chunk),
            Ok(None) => return Err("MCP server closed its stdout".to_string()),
            Err(err) => return Err(err),
        }
    }
}

/// One JSON-RPC round trip over the pipe.
pub fn round_trip(
    io: &dyn Stdio,
    buf: &mut Vec<u8>,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&request)
        .map_err(|err| format!("cannot encode MCP request: {err}"))?;
    line.push('\n');
    io.write_all(line.as_bytes())?;
    let answer = read_line(io, buf)?;
    let message: serde_json::Value = serde_json::from_str(&answer)
        .map_err(|err| format!("MCP server answered bad JSON: {err}"))?;
    if let Some(error) = message.get("error") {
        return Err(format!("MCP server errored: {error}"));
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| format!("MCP server answered without a result: {answer}"))
}

/// The live side of one spawned server: `initialize`, the
/// `notifications/initialized` handshake, `tools/list`, `tools/call`.
pub struct Session<T: Stdio> {
    io: T,
    buf: Vec<u8>,
    next_id: u64,
}

impl<T: Stdio> Session<T> {
    /// Wrap a spawned server's pipe.
    pub fn new(io: T) -> Session<T> {
        Session {
            io,
            buf: Vec::new(),
            next_id: 1,
        }
    }

    fn rpc(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        round_trip(&self.io, &mut self.buf, id, method, params)
    }

    /// The MCP opening handshake: `initialize`, then the client must
    /// send `notifications/initialized` before any other call.
    pub fn initialize(&mut self) -> Result<(), String> {
        let result = self.rpc(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "lca", "version": "0.6.0"},
            }),
        )?;
        if result.get("protocolVersion").is_none() {
            return Err("MCP server omitted its protocol version".to_string());
        }
        let note = b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";
        self.io.write_all(note)
    }

    /// List the server's tools, qualified for `server`.
    pub fn tools(&mut self, server: &str) -> Result<Vec<ServerTool>, String> {
        let result = self.rpc("tools/list", serde_json::json!({}))?;
        tools_from_list(server, &result)
    }
}

/// Map one `tools/list` result to qualified tools (both transports
/// share this, so stdio and HTTP name tools identically).
pub fn tools_from_list(
    server: &str,
    result: &serde_json::Value,
) -> Result<Vec<ServerTool>, String> {
    let empty = Vec::new();
    let listed = result
        .get("tools")
        .and_then(|tools| tools.as_array())
        .unwrap_or(&empty);
    let mut tools = Vec::with_capacity(listed.len());
    for entry in listed {
        let name = entry
            .get("name")
            .and_then(|name| name.as_str())
            .ok_or_else(|| "MCP server listed an unnamed tool".to_string())?;
        let annotations = entry.get("annotations");
        tools.push(ServerTool {
            qualified: tool_name(server, name),
            name: name.to_string(),
            description: entry
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string(),
            parameters: entry
                .get("inputSchema")
                .cloned()
                .unwrap_or(serde_json::json!({"type": "object"})),
            annotations: lca_protocol::ToolAnnotations {
                read_only_hint: annotations
                    .and_then(|a| a.get("readOnlyHint"))
                    .and_then(|hint| hint.as_bool()),
                destructive_hint: annotations
                    .and_then(|a| a.get("destructiveHint"))
                    .and_then(|hint| hint.as_bool()),
                idempotent_hint: annotations
                    .and_then(|a| a.get("idempotentHint"))
                    .and_then(|hint| hint.as_bool()),
                open_world_hint: annotations
                    .and_then(|a| a.get("openWorldHint"))
                    .and_then(|hint| hint.as_bool()),
            },
        });
    }
    Ok(tools)
}

impl<T: Stdio> Session<T> {
    /// Call one server tool by its server-side name (split impl: the
    /// listing mapper above is shared with the HTTP transport).
    pub fn call(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<ToolOutcome, String> {
        let result = self.rpc(
            "tools/call",
            serde_json::json!({"name": name, "arguments": arguments}),
        )?;
        Ok(outcome_from_result(&result))
    }
}

/// Map one `tools/call` result to text (both transports share this).
pub fn outcome_from_result(result: &serde_json::Value) -> ToolOutcome {
    let empty: Vec<serde_json::Value> = Vec::new();
    let content = result
        .get("content")
        .and_then(|content| content.as_array())
        .unwrap_or(&empty);
    let mut text = String::new();
    for item in content {
        let is_text = item.get("type").and_then(|kind| kind.as_str()) == Some("text");
        if let Some(part) = is_text
            .then(|| item.get("text"))
            .flatten()
            .and_then(|part| part.as_str())
        {
            text.push_str(part);
        } else {
            text.push_str("[non-text content omitted]");
        }
    }
    ToolOutcome {
        error: result
            .get("isError")
            .and_then(|flag| flag.as_bool())
            .unwrap_or(false),
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #53 - the name rule and the server-name rule hold at
    // the unit level (the bridge test pins them through a live server).
    #[test]
    fn names_follow_pi() {
        assert_eq!(tool_name("echo", "echo"), "mcp__echo__echo");
        assert!(is_valid_server_name("my-server_2"));
        assert!(!is_valid_server_name("has space"));
        assert!(!is_valid_server_name(""));
        assert!(
            parse_servers(r#"[{"name":"bad name","command":"x","cwd_scope":"workspace"}]"#)
                .is_err()
        );
    }

    // Verifies: gh #53 - the phase-2 envelope shapes hold at the unit
    // level (the remote tests pin them through the mock).
    #[test]
    fn config_shapes_hold() {
        assert!(is_transient_status(503));
        assert!(is_transient_status(429));
        assert!(!is_transient_status(501));
        assert!(!is_transient_status(400));
        assert_eq!(
            merge_scopes(&[Some("base"), Some("extra base"), None]),
            Some("base extra".to_string())
        );
        let challenge = parse_www_authenticate(
            r#"Bearer resource_metadata="https://x.invalid/m", scope="extra""#,
        );
        assert_eq!(challenge.get("scope").map(String::as_str), Some("extra"));
        let server = HttpServerConfig {
            name: "r".to_string(),
            url: "http://127.0.0.1:1/mcp".to_string(),
            headers: [("Authorization".to_string(), "Bearer x".to_string())]
                .into_iter()
                .collect(),
            timeout_secs: 0,
            oauth: None,
        };
        assert!(server.has_static_auth());
        assert_eq!(server.timeout(), std::time::Duration::from_secs(60));
    }

    // Verifies: gh #53 - a sanitized collision refuses the server with
    // both tool names in the error.
    #[test]
    fn collisions_name_both_tools() {
        let tools = vec![
            ServerTool {
                name: "do.thing".to_string(),
                qualified: tool_name("s", "do.thing"),
                description: String::new(),
                parameters: serde_json::json!({}),
                annotations: lca_protocol::ToolAnnotations {
                    read_only_hint: None,
                    destructive_hint: None,
                    idempotent_hint: None,
                    open_world_hint: None,
                },
            },
            ServerTool {
                name: "do_thing".to_string(),
                qualified: tool_name("s", "do_thing"),
                description: String::new(),
                parameters: serde_json::json!({}),
                annotations: lca_protocol::ToolAnnotations {
                    read_only_hint: None,
                    destructive_hint: None,
                    idempotent_hint: None,
                    open_world_hint: None,
                },
            },
        ];
        let err = qualify_tools("s", tools, crate::config::McpExposure::Direct, &[])
            .expect_err("collision refuses");
        assert!(
            err.contains("do.thing") && err.contains("do_thing"),
            "{err}"
        );
    }
}
