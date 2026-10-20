//! `mcp.json` (gh #53 phase 3): user and project configuration with
//! pi's merge rules, plus the file loading and persistence built on them
//! (gh #418 Phase B moved both out of `crates/lca-cli/src/mcp.rs`, so the
//! subsystem owns its own domain). Parsing stays pure and shared; the file
//! helpers at the end of this module are the only part that touches the
//! filesystem.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    /// The `cwd` a stdio entry names, expanded but unresolved.
    ///
    /// `~` / `~/...` expands to the user's home; everything else is
    /// returned as written, for the spawn seam to resolve against the
    /// session cwd inside the `workspace` scope (gh #418 Phase C#C1).
    pub fn cwd_path(&self) -> Option<String> {
        let EntryKind::Stdio { cwd, .. } = &self.kind else {
            return None;
        };
        let cwd = cwd.as_deref()?.trim();
        if cwd.is_empty() {
            return None;
        }
        Some(expand_home(cwd))
    }

    /// The stdio transport config, if this entry is a stdio server.
    pub fn stdio_config(&self) -> Option<crate::ServerConfig> {
        match &self.kind {
            EntryKind::Stdio {
                command, args, env, ..
            } => Some(crate::ServerConfig {
                name: self.name.clone(),
                command: command.clone(),
                args: args.clone(),
                // Resolved against the session cwd at spawn time (gh #418
                // Phase C#C1); the transport config carries the raw form.
                cwd: self.cwd_path(),
                env: env.clone(),
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
        /// The directory the child runs in, when the entry names one
        /// (gh #418 Phase C#C1: pi's `cwd`, a path resolved against the
        /// session cwd with a leading `~` expanded). `None` means the
        /// session cwd. The path is resolved through the `workspace`
        /// scope at spawn time, so it cannot aim outside it.
        cwd: Option<String>,
        /// Environment variables added on top of the inherited
        /// environment (pi's rule: `env` *overlays* `process.env`; a
        /// child that lost `PATH` could not run anything).
        env: BTreeMap<String, String>,
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

/// Expand a leading `~` to the user's home directory (pi's `expandHome`).
///
/// Only a leading `~` or `~/` expands: `~user` names another account, which
/// a config file has no business addressing, so it is left alone and will
/// fail scope resolution rather than silently mean something else.
pub fn expand_home(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\')) {
        return path.to_string();
    }
    let Some(home) = home_dir() else {
        return path.to_string();
    };
    format!("{}{}", home.to_string_lossy(), rest)
}

/// The user's home directory, with the workspace's existing platform rule.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
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
    // gh #418 Phase C#C2: `my-server` and `my_server` normalize into one
    // `mcp__my_server` namespace, so the second would shadow the first's
    // tools. Refuse it and name both (pi's rule: "server \"a\" conflicts
    // with \"b\""), and skip only the loser so the rest still load.
    let mut namespaces: BTreeMap<String, String> = BTreeMap::new();
    for (name, entry) in servers {
        match parse_entry(name, entry, source) {
            Ok(entry) => {
                let namespace = crate::mcp_namespace(name);
                if let Some(first) = namespaces.get(&namespace) {
                    parsed.warnings.push(format!(
                        "server {name:?} skipped: its name conflicts with {first:?} (both normalize to `{namespace}`)"
                    ));
                    continue;
                }
                namespaces.insert(namespace, name.clone());
                parsed.servers.push(entry);
            }
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
            // gh #418 Phase C#C1: `env` is supported. Values expand
            // `${VAR}` exactly like `args` do, and the map is passed to
            // the child *alongside* the inherited environment.
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
            EntryKind::Stdio {
                command: expand_value(command)?.to_string(),
                args,
                // A path, not a scope name (gh #418 Phase C#C1).
                cwd: get_str("cwd").map(str::to_string),
                env: env.unwrap_or_default(),
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

// ------------------------------------------------------------------
// File locations, loading, and persistence (gh #418 Phase B: these moved
// here from `crates/lca-cli/src/mcp.rs`, so the subsystem owns its own
// domain rather than the host owning it).
// ------------------------------------------------------------------

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
    pub entries: Vec<ServerEntry>,
    /// One line per skipped entry or unreadable file.
    pub warnings: Vec<String>,
}

/// Load user entries plus the trusted project's (an untrusted
/// project's file is ignored silently, like the config file's).
pub fn load_entries(data_dir: &Path, cwd: &Path, trusted: bool) -> LoadedMcp {
    let mut warnings = Vec::new();
    let user_path = user_config_path(data_dir);
    let mut parsed = match std::fs::read_to_string(&user_path) {
        Ok(text) => parse_mcp_json(&text, ServerSource::User),
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
                apply_project_json(&mut parsed, &text);
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

/// The indentation a file already uses, or two spaces for a new one.
///
/// pi's rule (`config.ts`: `/^([ \t]+)\S/m`): keep the shape the user
/// wrote, so an edit never reformats a whole config file. gh #418 Phase
/// C#C4.
fn indentation_of(text: &str) -> &str {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed == line {
            continue;
        }
        let indent_len = line.len() - trimmed.len();
        let indent = &line[..indent_len];
        // A tab is one level; spaces are whatever the file uses.
        return if indent.contains('\t') { "\t" } else { indent };
    }
    "  "
}

/// Serialize `value` with `indent` at every level.
fn to_string_with_indent(value: &serde_json::Value, indent: &str) -> Result<String, String> {
    let mut buffer = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    serde::Serialize::serialize(value, &mut serializer)
        .map_err(|err| format!("cannot encode: {err}"))?;
    String::from_utf8(buffer).map_err(|err| format!("cannot encode: {err}"))
}

/// Write one JSON file atomically, owner-only, keeping its indentation.
///
/// The house pattern (`lca-tools` `state_write`, `write_credentials`): write
/// a sibling temporary file, restrict it to the owner, then `rename` over
/// the target. A crash between the write and the rename leaves the previous
/// file intact - a half-written `mcp.json` is a config the user loses.
///
/// `mcp.json` carries credentials in `env` and `headers` by design, so the
/// process umask must not decide who can read it: `0600` on Unix, an
/// owner-only DACL on Windows. gh #418 Phase C#C4 (gh #267).
fn write_json_file(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{err}"))?;
    }
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut text = to_string_with_indent(value, indentation_of(&existing))?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let temp = sibling_temp_path(path);
    std::fs::write(&temp, text).map_err(|err| format!("{err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("cannot restrict {temp:?}: {err}"))?;
    }
    std::fs::rename(&temp, path).map_err(|err| {
        let _ = std::fs::remove_file(&temp);
        format!("{err}")
    })
}

/// A dot-prefixed sibling of `path`, so a leftover from a crash is not
/// mistaken for a real config file (the naming rule gh #320 established for
/// state writes).
fn sibling_temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mcp.json".to_string());
    path.with_file_name(format!(".{name}.tmp"))
}

/// Serialize one entry back to its file shape.
fn entry_json(entry: &ServerEntry) -> serde_json::Value {
    let mut value = serde_json::json!({
        "enabled": entry.enabled,
        "exposure": match entry.exposure {
            McpExposure::Direct => "direct",
            McpExposure::Codemode => "codemode",
            McpExposure::Deferred => "deferred",
            McpExposure::Hidden => "hidden",
        },
    });
    match &entry.kind {
        EntryKind::Stdio {
            command,
            args,
            cwd,
            env,
        } => {
            value["command"] = command.clone().into();
            value["args"] = args.clone().into();
            // gh #418 Phase C#C1: `cwd` and `env` must survive an edit
            // round-trip, or the next `/mcp` verb would delete them.
            if let Some(cwd) = cwd {
                value["cwd"] = cwd.clone().into();
            }
            if !env.is_empty() {
                value["env"] = env
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone().into()))
                    .collect::<serde_json::Map<String, serde_json::Value>>()
                    .into();
            }
        }
        EntryKind::Http {
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
                    McpExposure::Direct => "direct",
                    McpExposure::Codemode => "codemode",
                    McpExposure::Deferred => "deferred",
                    McpExposure::Hidden => "hidden",
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
pub(crate) fn write_full_entry(path: &Path, entry: &ServerEntry) -> Result<(), String> {
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

/// Remove one server from `path`, keeping every other entry and every
/// unknown key. `Ok(false)` means the file held no such server.
///
/// gh #418 Phase F: `lca mcp remove` reports the scope it did *not* find the
/// server in, so the caller distinguishes "removed" from "lives elsewhere".
pub(crate) fn remove_entry(path: &Path, name: &str) -> Result<bool, String> {
    if !path.is_file() {
        return Ok(false);
    }
    let mut file = read_json_file(path)?;
    let Some(servers) = file
        .get_mut("mcpServers")
        .and_then(|servers| servers.as_object_mut())
    else {
        return Ok(false);
    };
    if servers.remove(name).is_none() {
        return Ok(false);
    }
    write_json_file(path, &file)?;
    Ok(true)
}

/// Whether `path` defines a full entry (or an override) for `name`.
pub fn file_defines(path: &Path, name: &str) -> bool {
    if !path.is_file() {
        return false;
    }
    read_json_file(path)
        .ok()
        .and_then(|file| {
            file.get("mcpServers")
                .and_then(|servers| servers.get(name))
                .map(|_| true)
        })
        .unwrap_or(false)
}

/// Write a knob-only project override (a user-level server edited
/// under a trusted project), merging with whatever the file holds.
pub(crate) fn write_project_override(
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
