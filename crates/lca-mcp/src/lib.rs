//! The MCP subsystem (gh #53, gh #418): external MCP servers over stdio
//! and streamable HTTP, exposed as one tool suite.
//!
//! **This is a first-class internal subsystem, not an extension**
//! (ADR-0047, superseding ADR-0045). It lives in `crates/` with the rest of
//! the agent rather than in `extensions/`, because the extension boundary was
//! fiction in three ways: `lca-cli` carried it as a hard dependency, the host
//! owned its files and its permission prompting, and its WASM twin could not
//! serve the feature set it shipped (the `tool-catalog` world exports no
//! `net`, `oauth`, or `credentials`).
//!
//! What is preserved from the extension-bridge era, deliberately:
//!
//! - The **star topology**: one long-lived child per configured server,
//!   newline-delimited JSON-RPC over its stdio, and a single hop through
//!   streamable HTTP for remote ones.
//! - **Per-spawn permission prompts**. Spawning a server asks exactly as a
//!   model-requested command does, so a declined server never starts and the
//!   denial is recorded. Per-call arguments to an already-approved server do
//!   not re-ask.
//! - The **permission layer on every call**: every invocation runs through
//!   the turn's `tool_call`/`tool_result` hooks like any other tool.
//! - The **`!command` refusal**, which keeps shell syntax out of a tool name.
//!
//! Layout: this module is the protocol core and the domain types; `config` is
//! the `mcp.json` shape plus its file loading and persistence; `manager` is the
//! session lifecycle, grant assembly, `/mcp` action dispatch, and the
//! system-prompt section; `native` is the `ExtensionDispatch` implementation the
//! host drives; `remote` is the streamable-HTTP and OAuth path; `wire` is the
//! pure wire-protocol kit (SSE envelope, JSON-RPC selector, base64).

pub mod config;
pub mod manager;
pub mod native;
pub mod remote;
pub mod wire;
pub use manager::McpManager;
pub use native::{McpBridge, ServerStateKind, ServerStatus, manifest_grants};
pub use remote::{TokenStore, refresh, sign_in};

use std::collections::BTreeMap;

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

/// One listed resource (pi's `list_mcp_resources` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResource {
    /// The address `read_mcp_resource` reads.
    pub uri: String,
    /// The display name.
    pub name: String,
    /// The longer title, when listed.
    pub title: Option<String>,
    /// One line about it, when listed.
    pub description: Option<String>,
    /// The content type, when listed.
    pub mime_type: Option<String>,
}

/// One listed URI template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceTemplate {
    /// The template (`echo://{name}`).
    pub uri_template: String,
    /// The display name.
    pub name: String,
    /// The longer title, when listed.
    pub title: Option<String>,
    /// One line about it, when listed.
    pub description: Option<String>,
    /// The content type, when listed.
    pub mime_type: Option<String>,
}

/// One read resource content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceContent {
    /// Text (or text-like) content.
    Text {
        /// Where it was read from.
        uri: String,
        /// The listed type, when sent.
        mime_type: Option<String>,
        /// The text itself.
        text: String,
    },
    /// Binary content (base64 on the wire, bytes here).
    Blob {
        /// Where it was read from.
        uri: String,
        /// The listed type, when sent.
        mime_type: Option<String>,
        /// The decoded bytes.
        bytes: Vec<u8>,
    },
}

/// Map one `resources/list` result to resources plus the next cursor.
pub fn resources_from_list(result: &serde_json::Value) -> (Vec<McpResource>, Option<String>) {
    let empty = Vec::new();
    let listed = result
        .get("resources")
        .and_then(|resources| resources.as_array())
        .unwrap_or(&empty);
    let text = |entry: &serde_json::Value, key: &str| {
        entry
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    let resources = listed
        .iter()
        .filter_map(|entry| {
            Some(McpResource {
                uri: text(entry, "uri")?,
                name: text(entry, "name").unwrap_or_default(),
                title: text(entry, "title"),
                description: text(entry, "description"),
                mime_type: text(entry, "mimeType"),
            })
        })
        .collect();
    let cursor = result
        .get("nextCursor")
        .and_then(|cursor| cursor.as_str())
        .map(str::to_string);
    (resources, cursor)
}

/// Map one `resources/templates/list` result to templates (a server
/// without the method answers method-not-found; the sessions turn
/// that into an empty list, pi's `withoutTemplates`).
pub fn templates_from_list(result: &serde_json::Value) -> Vec<McpResourceTemplate> {
    let empty = Vec::new();
    let listed = result
        .get("resourceTemplates")
        .and_then(|templates| templates.as_array())
        .unwrap_or(&empty);
    let text = |entry: &serde_json::Value, key: &str| {
        entry
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    listed
        .iter()
        .filter_map(|entry| {
            Some(McpResourceTemplate {
                uri_template: text(entry, "uriTemplate")?,
                name: text(entry, "name").unwrap_or_default(),
                title: text(entry, "title"),
                description: text(entry, "description"),
                mime_type: text(entry, "mimeType"),
            })
        })
        .collect()
}

/// Map one `resources/read` result to contents.
pub fn contents_from_read(result: &serde_json::Value) -> Result<Vec<ResourceContent>, String> {
    let empty = Vec::new();
    let listed = result
        .get("contents")
        .and_then(|contents| contents.as_array())
        .unwrap_or(&empty);
    let mut out = Vec::with_capacity(listed.len());
    for entry in listed {
        let uri = entry
            .get("uri")
            .and_then(|uri| uri.as_str())
            .unwrap_or("")
            .to_string();
        let mime_type = entry
            .get("mimeType")
            .and_then(|mime| mime.as_str())
            .map(str::to_string);
        if let Some(text) = entry.get("text").and_then(|text| text.as_str()) {
            out.push(ResourceContent::Text {
                uri,
                mime_type,
                text: text.to_string(),
            });
        } else if let Some(blob) = entry.get("blob").and_then(|blob| blob.as_str()) {
            out.push(ResourceContent::Blob {
                uri,
                mime_type,
                bytes: crate::wire::base64_decode(blob)?,
            });
        } else {
            return Err("the server sent unreadable resource content".to_string());
        }
    }
    Ok(out)
}

/// The bridge's resource tools (pi's names): they reach every
/// connected, non-hidden server that offers resources.
pub const LIST_RESOURCES_TOOL: &str = "list_mcp_resources";
/// The bridge's template lister.
pub const LIST_TEMPLATES_TOOL: &str = "list_mcp_resource_templates";
/// The bridge's resource reader.
pub const READ_RESOURCE_TOOL: &str = "read_mcp_resource";

/// The three resource tools at one exposure.
pub fn resource_tool_specs(exposure: lca_protocol::ToolExposure) -> Vec<lca_protocol::ToolSpec> {
    let spec =
        |name: &str, description: &str, parameters: serde_json::Value| lca_protocol::ToolSpec {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
            exposure,
            namespace: None,
            annotations: None,
            extras: BTreeMap::new(),
        };
    vec![
        spec(
            LIST_RESOURCES_TOOL,
            "List MCP resources as JSON. With `server`, one page (`cursor` continues); without, the first page of every server.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string"},
                    "cursor": {"type": "string"},
                },
            }),
        ),
        spec(
            LIST_TEMPLATES_TOOL,
            "List MCP resource URI templates. With `server`, one server's; without, every server's.",
            serde_json::json!({
                "type": "object",
                "properties": {"server": {"type": "string"}},
            }),
        ),
        spec(
            READ_RESOURCE_TOOL,
            "Read one MCP resource by `server` and `uri`. Text arrives as text, images as images, other bytes as a file path.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string"},
                    "uri": {"type": "string"},
                },
                "required": ["server", "uri"],
            }),
        ),
    ]
}

/// The widest exposure among resource-serving servers (pi's rule:
/// `direct`, then `codemode` or `deferred`). `None` registers no
/// resource tools.
pub fn widest_resource_exposure(
    exposures: &[lca_protocol::ToolExposure],
) -> Option<lca_protocol::ToolExposure> {
    use lca_protocol::ToolExposure as Exposure;
    [Exposure::Direct, Exposure::Codemode, Exposure::Deferred]
        .into_iter()
        .find(|level| exposures.contains(level))
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
    // gh #291: the reply is the first message carrying OUR id. Server
    // notifications (`method`, no `id`) are ordinary MCP traffic and are
    // discarded; a foreign id fails loud because a single-outstanding-
    // request transport cannot see one legitimately. The selection rules
    // live in `crate::wire` (ADR-0047: selectors in the wire kit, transports
    // in this crate) so the stdio and HTTP paths agree.
    match crate::wire::response_for(id, || Ok(Some(read_line(io, buf)?)))? {
        crate::wire::Reply::Result(result) => Ok(result),
        crate::wire::Reply::Error(error) => Err(error),
    }
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
    /// Returns whether the server offers resources.
    pub fn initialize(&mut self) -> Result<bool, String> {
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
        let offers = result
            .get("capabilities")
            .and_then(|capabilities| capabilities.get("resources"))
            .is_some();
        let note = b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";
        self.io.write_all(note)?;
        Ok(offers)
    }

    /// List one page of resources plus the next cursor.
    pub fn resource_list(
        &mut self,
        cursor: Option<&str>,
    ) -> Result<(Vec<McpResource>, Option<String>), String> {
        let mut params = serde_json::json!({});
        if let Some(cursor) = cursor {
            params["cursor"] = cursor.into();
        }
        let result = self.rpc("resources/list", params)?;
        Ok(resources_from_list(&result))
    }

    /// List the URI templates (an unimplemented method reads as no
    /// templates, never an error).
    pub fn resource_templates(&mut self) -> Result<Vec<McpResourceTemplate>, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "resources/templates/list",
            "params": {},
        });
        let mut line = serde_json::to_string(&request)
            .map_err(|err| format!("cannot encode MCP request: {err}"))?;
        line.push('\n');
        self.io.write_all(line.as_bytes())?;
        // gh #291: same reader as `round_trip`, so notifications are
        // tolerated here too. The distinction that matters: a JSON-RPC
        // error for our id means the method is unimplemented, which reads
        // as "no templates", while a transport failure (bad JSON, EOF, a
        // desynchronized id) propagates - a corrupt stream must never
        // masquerade as an empty listing.
        match crate::wire::response_for(id, || Ok(Some(read_line(&self.io, &mut self.buf)?)))? {
            crate::wire::Reply::Result(result) => Ok(templates_from_list(&result)),
            crate::wire::Reply::Error(_) => Ok(Vec::new()),
        }
    }

    /// Read one resource by URI.
    pub fn resource_read(&mut self, uri: &str) -> Result<Vec<ResourceContent>, String> {
        let result = self.rpc("resources/read", serde_json::json!({"uri": uri}))?;
        contents_from_read(&result)
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

#[cfg(test)]
mod notification_tolerance_tests {
    //! gh #291: JSON-RPC notifications must not desync the stdio stream.
    //!
    //! MCP servers emit server-to-client notifications as ordinary
    //! traffic (`notifications/message`, `notifications/progress`,
    //! `notifications/tools/list_changed`) between the request and its
    //! reply. A reader that takes the next line as the answer parses a
    //! notification as the reply — and because the *real* reply stays
    //! queued in `buf`, every later call then reads a stale line and the
    //! connection is permanently desynchronized.
    //!
    //! These rows drive the real `round_trip`/`Session` over a scripted
    //! line source, which is the whole transport the fix touches.

    use super::*;
    use std::cell::RefCell;

    /// A [`Stdio`] that hands out pre-scripted lines and records what
    /// was written. Reads are line-granular so a row's script reads
    /// exactly like the wire.
    struct ScriptedIo {
        incoming: RefCell<Vec<String>>,
        written: RefCell<Vec<String>>,
    }

    impl ScriptedIo {
        fn new(lines: &[&str]) -> ScriptedIo {
            ScriptedIo {
                incoming: RefCell::new(lines.iter().map(|line| (*line).to_string()).collect()),
                written: RefCell::new(Vec::new()),
            }
        }
    }

    impl Stdio for ScriptedIo {
        fn write_all(&self, bytes: &[u8]) -> Result<(), String> {
            self.written
                .borrow_mut()
                .push(String::from_utf8_lossy(bytes).into_owned());
            Ok(())
        }

        fn read_some(&self, _max: usize) -> Result<Option<Vec<u8>>, String> {
            let next = {
                let mut queue = self.incoming.borrow_mut();
                if queue.is_empty() {
                    None
                } else {
                    Some(queue.remove(0))
                }
            };
            Ok(next.map(|line| {
                let mut bytes = line.into_bytes();
                bytes.push(b'\n');
                bytes
            }))
        }
    }

    fn reply(id: u64, result: serde_json::Value) -> String {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
    }

    fn notification(method: &str) -> String {
        // Notifications carry `method` and NO `id`: that is the shape
        // the reader must discard.
        serde_json::json!({"jsonrpc": "2.0", "method": method}).to_string()
    }

    // Verifies: gh #291 - a notification arriving BEFORE the reply is
    // discarded and the reply still answers the call.
    #[test]
    fn a_notification_before_the_reply_is_skipped() {
        let io = ScriptedIo::new(&[
            &notification("notifications/message"),
            &reply(1, serde_json::json!({"ok": true})),
        ]);
        let mut buf = Vec::new();
        let answer = round_trip(&io, &mut buf, 1, "tools/call", serde_json::json!({}))
            .expect("the reply is found past the notification");
        assert_eq!(answer, serde_json::json!({"ok": true}));
    }

    // Verifies: gh #291 - several notifications of different kinds,
    // interleaved with the reply, are all discarded.
    #[test]
    fn every_notification_kind_is_skipped() {
        let io = ScriptedIo::new(&[
            &notification("notifications/message"),
            &notification("notifications/progress"),
            &notification("notifications/tools/list_changed"),
            &reply(4, serde_json::json!({"tools": []})),
        ]);
        let mut buf = Vec::new();
        let answer = round_trip(&io, &mut buf, 4, "tools/list", serde_json::json!({}))
            .expect("every notification is skipped");
        assert_eq!(answer, serde_json::json!({"tools": []}));
    }

    // Verifies: gh #291 - THE DESYNC PIN. Two sequential calls on one
    // session, each preceded by a notification: call 2 must get its own
    // reply, never call 1's. On the pre-fix reader call 1 consumed the
    // notification and failed, leaving call 1's real reply queued so
    // call 2 answered with call 1's payload.
    #[test]
    fn two_sequential_calls_stay_in_sync_across_notifications() {
        let io = ScriptedIo::new(&[
            // call 1's notification, then call 1's reply
            &notification("notifications/message"),
            &reply(1, serde_json::json!({"call": "first"})),
            // call 2's notification, then call 2's reply
            &notification("notifications/progress"),
            &reply(2, serde_json::json!({"call": "second"})),
        ]);
        let mut buf = Vec::new();
        let first = round_trip(&io, &mut buf, 1, "tools/call", serde_json::json!({}))
            .expect("first call answers");
        assert_eq!(first, serde_json::json!({"call": "first"}));
        let second = round_trip(&io, &mut buf, 2, "tools/call", serde_json::json!({}))
            .expect("second call answers");
        assert_eq!(
            second,
            serde_json::json!({"call": "second"}),
            "the second call must not read the first call's reply"
        );
    }

    // Verifies: gh #291 - a reply carrying SOMEONE ELSE'S id is a
    // desync, not noise: a single-outstanding-request transport cannot
    // legitimately receive a foreign id, so it fails loud and names it
    // rather than silently consuming a reply that belongs elsewhere.
    #[test]
    fn a_foreign_id_fails_loud_and_names_it() {
        let io = ScriptedIo::new(&[&reply(99, serde_json::json!({"wrong": true}))]);
        let mut buf = Vec::new();
        let err = round_trip(&io, &mut buf, 1, "tools/call", serde_json::json!({}))
            .expect_err("a foreign id is a desync");
        assert!(err.contains("99"), "the foreign id is named: {err}");
    }

    // Verifies: gh #291 - an error for OUR id is the server's answer
    // and keeps today's error shape.
    #[test]
    fn an_error_for_our_id_returns_the_error_payload() {
        let io = ScriptedIo::new(&[
            &notification("notifications/message"),
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 7,
                "error": {"code": -32601, "message": "unknown method"},
            })
            .to_string(),
        ]);
        let mut buf = Vec::new();
        let err = round_trip(&io, &mut buf, 7, "tools/call", serde_json::json!({}))
            .expect_err("the error is the answer");
        assert!(err.contains("unknown method"), "{err}");
    }

    // Verifies: gh #291 - bad JSON stays a hard error: that is real
    // corruption, not a notification.
    #[test]
    fn bad_json_is_still_a_hard_error() {
        let io = ScriptedIo::new(&["this is not json"]);
        let mut buf = Vec::new();
        let err = round_trip(&io, &mut buf, 1, "tools/call", serde_json::json!({}))
            .expect_err("bad JSON is corruption");
        assert!(err.contains("bad JSON"), "{err}");
    }

    // Verifies: gh #291 - notification-only traffic never reports
    // success: it keeps waiting and ends on EOF with a failure.
    #[test]
    fn notification_only_traffic_never_succeeds() {
        let io = ScriptedIo::new(&[
            &notification("notifications/message"),
            &notification("notifications/progress"),
        ]);
        let mut buf = Vec::new();
        let err = round_trip(&io, &mut buf, 1, "tools/call", serde_json::json!({}))
            .expect_err("no reply means no success");
        assert!(err.contains("closed its stdout"), "{err}");
    }

    // Verifies: gh #291 - `resource_templates` (the second reader)
    // tolerates notifications the same way instead of reporting
    // "answered another call".
    #[test]
    fn resource_templates_skip_notifications() {
        let io = ScriptedIo::new(&[
            &notification("notifications/tools/list_changed"),
            &reply(1, serde_json::json!({"resourceTemplates": []})),
        ]);
        let mut session = Session::new(io);
        let templates = session
            .resource_templates()
            .expect("a notification does not break the listing");
        assert!(templates.is_empty());
    }
}
