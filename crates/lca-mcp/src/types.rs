//! The domain types and the credential/policy predicates every transport
//! shares (gh #555: split from the crate root for the line ceiling).
//!
//! Split, not moved in ownership: the crate root still re-exports all of it
//! (`lca_mcp::ServerConfig`, `lca_mcp::StoredToken`, …), so no caller's path
//! changes. What lives here is the **data** an `mcp.json` entry or an OAuth
//! credential is made of, plus the predicates that decide what a listed server
//! (`is_valid_server_name`, `parse_servers`), a credential scope (`merge_scopes`,
//! `token_key`), a challenge header (`parse_www_authenticate`), or an HTTP status
//! (`is_transient_status`) means.
//!
//! Naming (`mcp_namespace`, `tool_name`, `create_tool_name`) stays in the crate
//! root, next to `qualified_pairs`: the rules there are one decision about how a
//! server's tools are spelled, and splitting them from the code that applies them
//! would be two places to keep in step.

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
    /// The directory the child runs in, relative to the session cwd
    /// (pi's `cwd`); `None` means the session cwd itself.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Environment variables overlaid on the inherited environment
    /// (pi's `env`).
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
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
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
                // RFC 7235 auth-params are case-insensitive, and RFC 9728's
                // member is spelled `resource_metadata`. Lowercasing here is what
                // lets every reader ask for it by its canonical spelling without
                // each one re-deciding how to compare - a server that sends
                // `Resource_Metadata` is not misconfigured, it is exercising the
                // freedom the grammar gives it (gh #290).
                params.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
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
