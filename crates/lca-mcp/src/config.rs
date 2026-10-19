//! `mcp.json` (gh #53 phase 3): user and project configuration with
//! pi's merge rules. Pure parsing - no files, no host - so the bridge,
//! the host loader, and the tests share every refusal.

use std::collections::BTreeMap;

/// Where one entry came from (the manager persists edits to the
/// defining file; project overrides apply to the session per pi).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerSource {
    /// The user's file.
    User,
    /// The trusted project's file.
    Project,
    /// Constructed in code, never persisted (phase-1/2 constructors).
    Inline,
}

/// How the model reaches one server's tools (pi's `exposure`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpExposure {
    /// Callable from discovery only (pi's default).
    #[default]
    Codemode,
    /// Found through `tool_search`, then called directly.
    Deferred,
    /// Declared to the model like a built-in tool.
    Direct,
    /// Registered but unreachable.
    Hidden,
}

impl McpExposure {
    /// Parse the wire string; `codemode-deferred` is pi's alias for
    /// `codemode`. Unknown values refuse with the value itself.
    pub fn parse(value: &str) -> Result<McpExposure, String> {
        match value {
            "codemode" | "codemode-deferred" => Ok(McpExposure::Codemode),
            "deferred" => Ok(McpExposure::Deferred),
            "direct" => Ok(McpExposure::Direct),
            "hidden" => Ok(McpExposure::Hidden),
            unknown => Err(format!("unknown MCP exposure `{unknown}`")),
        }
    }

    /// The host exposure one MCP level maps to.
    pub fn as_tool_exposure(self) -> lca_protocol::ToolExposure {
        match self {
            McpExposure::Codemode => lca_protocol::ToolExposure::Codemode,
            McpExposure::Deferred => lca_protocol::ToolExposure::Deferred,
            McpExposure::Direct => lca_protocol::ToolExposure::Direct,
            McpExposure::Hidden => lca_protocol::ToolExposure::Hidden,
        }
    }
}

/// One configured server.
#[derive(Debug, Clone)]
pub struct ServerEntry {
    /// pi's server name.
    pub name: String,
    /// How to reach it.
    pub kind: EntryKind,
    /// `false` keeps the entry without connecting.
    pub enabled: bool,
    /// The server-wide default exposure.
    pub exposure: McpExposure,
    /// Per-tool overrides in sorted order (exact names first at
    /// match time, then patterns in this order - pi uses file order,
    /// `serde_json` sorts keys, so this is the documented cut).
    pub tool_exposure: Vec<(String, McpExposure)>,
    /// One line for the system-prompt section.
    pub description: String,
    /// Which file defined it.
    pub source: ServerSource,
}

/// Convert one entry to its stdio transport config, if it has one.
impl ServerEntry {
    /// The stdio transport config, if this entry is a stdio server.
    pub fn stdio_config(&self) -> Option<crate::ServerConfig> {
        match &self.kind {
            EntryKind::Stdio {
                command,
                args,
                cwd_scope,
            } => Some(crate::ServerConfig {
                name: self.name.clone(),
                command: command.clone(),
                args: args.clone(),
                cwd_scope: cwd_scope.clone(),
            }),
            EntryKind::Http { .. } => None,
        }
    }

    /// The HTTP transport config, if this entry is a remote server.
    pub fn http_config(&self) -> Option<crate::HttpServerConfig> {
        match &self.kind {
            EntryKind::Http {
                url,
                headers,
                timeout_secs,
                oauth,
            } => Some(crate::HttpServerConfig {
                name: self.name.clone(),
                url: url.clone(),
                headers: headers.clone(),
                timeout_secs: *timeout_secs,
                oauth: oauth.clone(),
            }),
            EntryKind::Stdio { .. } => None,
        }
    }
}

/// The transport half of one entry.
#[derive(Debug, Clone)]
pub enum EntryKind {
    /// A stdio server (pi's `command`/`args`/`env`/`cwd`).
    Stdio {
        /// One executable (argv, never a shell string).
        command: String,
        /// Its arguments.
        args: Vec<String>,
        /// The `fs` scope it runs in (pi's `cwd`; a scope name, so an
        /// entry cannot aim outside what the manifest grants).
        cwd_scope: String,
    },
    /// A streamable-HTTP server (pi's `url`/`headers`/`oauth`).
    Http {
        /// The streamable-HTTP endpoint.
        url: String,
        /// Extra headers (an `Authorization` header disables OAuth).
        headers: BTreeMap<String, String>,
        /// Per-request timeout in seconds (zero means the default).
        timeout_secs: u64,
        /// OAuth settings (`None` means no sign-in flow configured).
        oauth: Option<crate::OAuthConfig>,
    },
}

/// What one file parsed to: servers plus the reported-and-skipped
/// rows (pi: invalid entries never block the valid ones).
#[derive(Debug, Default)]
pub struct ParsedConfig {
    /// The valid entries, sorted by name.
    pub servers: Vec<ServerEntry>,
    /// One line per skipped entry, naming the server and the cause.
    pub warnings: Vec<String>,
}

/// Expand `${NAME}` from the process environment. A value that IS a
/// `!command` is refused (pi executes it; executing config content is
/// a trust hole, so LCA loud-skips the entry instead - documented
/// divergence).
pub fn expand_value(value: &str) -> Result<String, String> {
    if value.starts_with('!') {
        return Err("`!command` values are not executed; use an environment variable".to_string());
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(format!("unclosed `${{` in {value:?}"));
        };
        let name = &after[..end];
        out.push_str(&std::env::var(name).map_err(|_| format!("{name} is not set"))?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `*` matches any run (pi's `toolExposure` patterns); everything
/// else is literal.
pub fn pattern_matches(pattern: &str, tool: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else {
        return true;
    };
    if !tool.starts_with(first) {
        return false;
    }
    let mut rest = &tool[first.len()..];
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let Some(found) = rest.find(part) else {
            return false;
        };
        rest = &rest[found + part.len()..];
    }
    pattern.ends_with('*') || rest.is_empty()
}

/// Resolve one tool's exposure: exact names win, then patterns in
/// order (pi's rule, minus file order - see [`ServerEntry`]).
pub fn exposure_for(
    default: McpExposure,
    rules: &[(String, McpExposure)],
    tool: &str,
) -> McpExposure {
    if let Some((_, exposure)) = rules.iter().find(|(name, _)| name == tool) {
        return *exposure;
    }
    rules
        .iter()
        .find(|(pattern, _)| pattern_matches(pattern, tool))
        .map(|(_, exposure)| *exposure)
        .unwrap_or(default)
}

/// Parse one `mcp.json` file's text.
pub fn parse_mcp_json(text: &str, source: ServerSource) -> ParsedConfig {
    let mut parsed = ParsedConfig::default();
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            parsed
                .warnings
                .push(format!("the file does not parse: {err}"));
            return parsed;
        }
    };
    let empty = serde_json::Map::new();
    let servers = value
        .get("mcpServers")
        .and_then(|servers| servers.as_object())
        .unwrap_or(&empty);
    for (name, entry) in servers {
        match parse_entry(name, entry, source) {
            Ok(entry) => parsed.servers.push(entry),
            Err(err) => parsed
                .warnings
                .push(format!("server {name:?} skipped: {err}")),
        }
    }
    parsed.servers.sort_by(|a, b| a.name.cmp(&b.name));
    parsed
}

/// Parse one full `mcpServers` entry (a transport is required; the
/// project override shape goes through [`apply_project_json`]).
fn parse_entry(
    name: &str,
    entry: &serde_json::Value,
    source: ServerSource,
) -> Result<ServerEntry, String> {
    if !crate::is_valid_server_name(name) {
        return Err("names hold letters, digits, `_`, `-` only".to_string());
    }
    let get_str = |key: &str| entry.get(key).and_then(|value| value.as_str());
    if let Some(kind) = get_str("type") {
        match kind {
            "stdio" | "http" | "streamable-http" => {}
            "sse" => return Err("the legacy SSE transport is not supported".to_string()),
            unknown => return Err(format!("unknown server type `{unknown}`")),
        }
    }
    let command = get_str("command");
    let url = get_str("url");
    let kind = match (command, url) {
        (Some(command), None) => {
            if command.is_empty() {
                return Err("the command is empty".to_string());
            }
            let args = entry
                .get("args")
                .and_then(|args| args.as_array())
                .unwrap_or(&Vec::new())
                .iter()
                .map(|arg| {
                    arg.as_str()
                        .ok_or_else(|| "args holds strings only".to_string())
                        .and_then(expand_value)
                })
                .collect::<Result<Vec<_>, _>>()?;
            // `env` parses and expands (shared validation), but a
            // non-empty map skips the entry: per-server environment
            // needs a process-env seam the frozen capability has no
            // slot for (0.7 cut line, named in the warning).
            let env = entry
                .get("env")
                .and_then(|env| env.as_object())
                .map(|env| {
                    env.iter()
                        .map(|(key, value)| {
                            value
                                .as_str()
                                .ok_or_else(|| format!("env {key:?} holds a string"))
                                .and_then(expand_value)
                                .map(|expanded| (key.clone(), expanded))
                        })
                        .collect::<Result<BTreeMap<_, _>, _>>()
                })
                .transpose()?;
            if env.as_ref().is_some_and(|env| !env.is_empty()) {
                return Err(
                    "env needs the 0.7 process-env seam; the entry waits for it".to_string()
                );
            }
            EntryKind::Stdio {
                command: expand_value(command)?.to_string(),
                args,
                cwd_scope: get_str("cwd").unwrap_or("workspace").to_string(),
            }
        }
        (None, Some(url)) => {
            let headers = entry
                .get("headers")
                .and_then(|headers| headers.as_object())
                .map(|headers| {
                    headers
                        .iter()
                        .map(|(key, value)| {
                            value
                                .as_str()
                                .ok_or_else(|| format!("header {key:?} holds a string"))
                                .and_then(expand_value)
                                .map(|expanded| (key.clone(), expanded))
                        })
                        .collect::<Result<BTreeMap<_, _>, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            let oauth = entry.get("oauth").map(parse_oauth).transpose()?;
            EntryKind::Http {
                url: url.to_string(),
                headers,
                timeout_secs: entry
                    .get("timeout")
                    .and_then(|timeout| timeout.as_u64())
                    .unwrap_or(0),
                oauth,
            }
        }
        (Some(_), Some(_)) => return Err("a server has a command or a url, never both".to_string()),
        (None, None) => {
            return Err("a server needs a command (stdio) or a url (streamable HTTP)".to_string());
        }
    };
    Ok(ServerEntry {
        name: name.to_string(),
        kind,
        enabled: entry
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        exposure: entry
            .get("exposure")
            .and_then(|v| v.as_str())
            .map(McpExposure::parse)
            .transpose()?
            .unwrap_or_default(),
        tool_exposure: entry
            .get("toolExposure")
            .and_then(|rules| rules.as_object())
            .map(|rules| {
                rules
                    .iter()
                    .map(|(tool, exposure)| {
                        exposure
                            .as_str()
                            .ok_or_else(|| format!("toolExposure {tool:?} holds a string"))
                            .and_then(McpExposure::parse)
                            .map(|exposure| (tool.clone(), exposure))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default(),
        description: get_str("description").unwrap_or("").to_string(),
        source,
    })
}

/// Parse one `oauth` object (the phase-2 subset).
fn parse_oauth(value: &serde_json::Value) -> Result<crate::OAuthConfig, String> {
    let get = |key: &str| value.get(key).and_then(|value| value.as_str());
    Ok(crate::OAuthConfig {
        client_name: get("clientName").map(str::to_string),
        scope: get("scope").map(str::to_string),
        auth_server_metadata_url: get("authServerMetadataUrl").map(str::to_string),
        client_id: get("clientId").map(str::to_string),
        client_secret: get("clientSecret")
            .map(expand_value)
            .transpose()?
            .map(|secret| secret.to_string()),
    })
}

/// Apply one project file onto the user entries (pi's rules): a
/// project entry WITH a transport adds or replaces the server; one
/// WITHOUT only overrides the session knobs (`enabled`, `exposure`,
/// `toolExposure`, `description`) of a user-level server and keeps
/// the rest, and later edits persist to the override. A partial
/// override for an unknown server is reported and skipped. Project
/// text only reaches this function when trusted - the caller (host
/// side) owns that gate, never the parser.
pub fn apply_project_json(user: &mut ParsedConfig, text: &str) {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            user.warnings
                .push(format!("the project file does not parse: {err}"));
            return;
        }
    };
    let empty = serde_json::Map::new();
    let servers = value
        .get("mcpServers")
        .and_then(|servers| servers.as_object())
        .unwrap_or(&empty);
    for (name, entry) in servers {
        let full = entry.get("command").is_some()
            || entry.get("url").is_some()
            || entry.get("type").is_some();
        if full {
            match parse_entry(name, entry, ServerSource::Project) {
                Ok(entry) => {
                    if let Some(known) = user
                        .servers
                        .iter_mut()
                        .find(|server| server.name == entry.name)
                    {
                        *known = entry;
                    } else {
                        user.servers.push(entry);
                    }
                }
                Err(err) => user
                    .warnings
                    .push(format!("project server {name:?} skipped: {err}")),
            }
            continue;
        }
        let Some(known) = user.servers.iter_mut().find(|server| server.name == *name) else {
            user.warnings.push(format!(
                "project server {name:?} skipped: it overrides no user-level server"
            ));
            continue;
        };
        if let Some(enabled) = entry.get("enabled").and_then(|v| v.as_bool()) {
            known.enabled = enabled;
        }
        if let Some(exposure) = entry.get("exposure").and_then(|v| v.as_str()) {
            match McpExposure::parse(exposure) {
                Ok(exposure) => known.exposure = exposure,
                Err(err) => user
                    .warnings
                    .push(format!("project server {name:?} skipped: {err}")),
            }
        }
        if let Some(rules) = entry.get("toolExposure").and_then(|v| v.as_object()) {
            match rules
                .iter()
                .map(|(tool, exposure)| {
                    exposure
                        .as_str()
                        .ok_or_else(|| format!("toolExposure {tool:?} holds a string"))
                        .and_then(McpExposure::parse)
                        .map(|exposure| (tool.clone(), exposure))
                })
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(rules) => known.tool_exposure = rules,
                Err(err) => user
                    .warnings
                    .push(format!("project server {name:?} skipped: {err}")),
            }
        }
        if let Some(description) = entry.get("description").and_then(|v| v.as_str()) {
            known.description = description.to_string();
        }
        // Later manager edits persist to the override, never back to
        // the user file (pi's rule).
        known.source = ServerSource::Project;
    }
    user.servers.sort_by(|a, b| a.name.cmp(&b.name));
}
