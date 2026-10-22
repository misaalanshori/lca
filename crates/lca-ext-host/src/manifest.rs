//! Split from `lib.rs` (cycle 7, P3). Behaviour unchanged.

use super::*;

/// A parsed `extension.toml`.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Extension identity; also its credential namespace later (FR-PERM-6).
    pub name: String,
    /// Extension release version.
    pub version: String,
    /// The ABI line, `major.minor`.
    pub abi: String,
    /// Worlds the component implements. Empty for a data-only extension
    /// (`worlds = []`, resources only; ADR-0030).
    pub worlds: Vec<String>,
    /// Declared `resources/` kinds (ADR-0030): the installer refuses an
    /// undeclared kind; the host serves the bag by convention.
    pub resources: Vec<String>,
    /// Declared `fs` scopes with their modes (FR-PERM-1).
    pub fs: Vec<ScopeGrant>,
    /// The `process` capability was declared (FR-PERM-1).
    pub process: bool,
    /// The `pty` capability was declared (FR-PERM-1).
    pub pty: bool,
    /// The `net` (or `net-local`) capability was declared at all (gh
    /// #302). `hosts = []` is the shape an extension ships when its
    /// hosts arrive by consent, so "declared with no hosts" must stay
    /// distinguishable from "never declared".
    pub net_declared: bool,
    /// `net` host patterns (internet, HTTPS only).
    pub net: Vec<lca_permissions::NetPattern>,
    /// `net-local` address patterns (local ranges, HTTP allowed).
    pub net_local: Vec<lca_permissions::LocalPattern>,
    /// The loopback OAuth flow settings, when declared.
    pub oauth: Option<OAuthSettings>,
    /// The `env` capability (gh #170): exact host-variable names this
    /// extension may read, forwarded into its `WasiCtx` at instantiation.
    ///
    /// Exact names only - a wildcard is refused at parse, because the names
    /// are the whole consent gate (the same reasoning `net`'s bare `*` got).
    /// `env_declared` keeps "declared with no names" distinguishable from
    /// "never declared", mirroring `net_declared` (gh #302).
    pub env: Vec<String>,
    /// The `env` capability was declared at all.
    pub env_declared: bool,
    /// The `serve-http` capability's manifest parameters (gh #363,
    /// ADR-0049), when declared. The listener itself is the host's; this is
    /// only what the extension claimed, so the consent line can name it.
    pub serve_http: Option<ServeHttpSettings>,
    /// The credential namespace, when declared; must equal `name`
    /// (FR-PERM-6, no cross-namespace read at any level, FR-PERM-7).
    pub credentials: bool,
    /// The `completion` capability was declared (ADR-0015).
    pub completion: bool,
    /// The `tools` capability was declared (gh #77): this extension
    /// may call other tools through the host's `tools` import.
    pub tools: bool,
    /// The `ui` regions this manifest declares (capability catalog's
    /// four-region enum; empty means no rendering rights at all).
    pub ui_regions: Vec<String>,
    /// The manifest's resource hints, clamped to the host's maxima at
    /// load (schema `limits`: memory64MB/fuel10M defaults,512MB/1B
    /// maximums; absent means the host's own values apply).
    pub limits: Option<ExtensionLimits>,
    /// The env var carrying a base-URL override for this provider's
    /// login (gh #157: `[login] env_base_url`, e.g.
    /// `OPENAI_BASE_URL`). Absent means the provider has no
    /// environment override the host should consult.
    pub login_env_base_url: Option<String>,
}

fn reason_of(value: &toml::Value, key: &str) -> Result<String, LoadError> {
    let reason = value
        .get("reason")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LoadError::InvalidManifest(format!("{key} needs a reason string")))?;
    if reason.len() < 10 {
        return Err(LoadError::InvalidManifest(format!(
            "{key}'s reason must say something a person can evaluate (at least10 characters)"
        )));
    }
    Ok(reason.to_string())
}

/// Read and validate the `worlds` list (gh #288).
///
/// Every entry must name a world in [`lca_ext_abi::WORLDS`]. Three things used to
/// slip through, and all three were silent:
///
/// 1. **A typo.** `worlds = ["contxt-transform"]` installed and loaded, and the
///    dispatcher's `filter_map` dropped it, so the extension matched no world and
///    did nothing - with a successful install and no error anywhere.
/// 2. **A non-string.** `filter_map(|item| item.as_str())` dropped `worlds = [1]`
///    and `worlds = ["tool", 7]` on the floor, so a malformed entry was not an
///    error but a silently shorter list.
/// 3. **A duplicate.** `["tool", "tool"]` was accepted and handed to the loader
///    twice; the schema already says `uniqueItems`, so the code was the half that
///    was wrong.
///
/// Refused rather than deduplicated for the duplicate case: a repeated world is
/// almost always a copy-paste mistake in a list the author is still writing, and
/// saying so is cheaper for them than silently getting one of the two.
fn parse_worlds(value: &toml::Value) -> Result<Vec<String>, LoadError> {
    let Some(items) = value.get("worlds").map(|worlds| {
        worlds.as_array().cloned().ok_or_else(|| {
            LoadError::InvalidManifest("`worlds` must be an array of world names".into())
        })
    }) else {
        return Ok(Vec::new());
    };
    let items = items?;
    let mut worlds: Vec<String> = Vec::with_capacity(items.len());
    for item in &items {
        let Some(name) = item.as_str() else {
            // Say what was written, not just that it was wrong: a `worlds` array
            // holding a number is usually a missing pair of quotes.
            return Err(LoadError::InvalidManifest(format!(
                "`worlds` entry {item} is not a string; each entry must be a quoted world name"
            )));
        };
        if lca_ext_abi::parse_world(name).is_none() {
            let hint = match lca_ext_abi::nearest_world(name) {
                Some(suggestion) => format!(" - did you mean `{suggestion}`?"),
                None => String::new(),
            };
            return Err(LoadError::InvalidManifest(format!(
                "unknown world `{name}`{hint} Known worlds: {}",
                lca_ext_abi::WORLDS
                    .iter()
                    .map(|(name, _)| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        if worlds.iter().any(|seen| seen == name) {
            return Err(LoadError::InvalidManifest(format!(
                "`worlds` lists `{name}` more than once; each world may appear once"
            )));
        }
        worlds.push(name.to_string());
    }
    Ok(worlds)
}

impl Manifest {
    /// Whether the manifest declares `world`.
    ///
    /// The loader asks this instead of comparing world strings itself (gh #288):
    /// a `world == "ui"` literal in the linking path was a second copy of the
    /// vocabulary, and a typo there - `"uii"` - would have been invisible,
    /// because validation would still accept the name the author wrote while the
    /// loader never matched it. Asking in terms of [`lca_ext_abi::World`] makes
    /// that typo a compile error instead of a silent no-op.
    ///
    /// Resolves through [`lca_ext_abi::WORLDS`], so a manifest built in code (the
    /// tests do this) is consulted the same way as a parsed one.
    #[must_use]
    pub fn declares(&self, world: lca_ext_abi::World) -> bool {
        self.worlds
            .iter()
            .any(|declared| lca_ext_abi::parse_world(declared) == Some(world))
    }

    /// Parse and validate identity and capability declarations (the
    /// schema's rules; full JSON-schema validation lands with Phase 5).
    pub fn parse(toml_text: &str) -> Result<Manifest, LoadError> {
        let value: toml::Value = toml::from_str(toml_text)
            .map_err(|err| LoadError::InvalidManifest(format!("not valid TOML: {err}")))?;
        let get = |key: &str| {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| LoadError::InvalidManifest(format!("missing `{key}`")))
        };
        let name = get("name")?;
        if !name.starts_with(|c: char| c.is_ascii_lowercase())
            || name.len() < 2
            || name.len() > 64
            || !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || name.contains("--")
            || name.ends_with('-')
        {
            return Err(LoadError::InvalidManifest(format!(
                "`name` {name:?} does not match the manifest identifier rules"
            )));
        }
        let version = get("version")?;
        let abi = get("abi")?;
        let worlds = parse_worlds(&value)?;
        let resources = value
            .get("resources")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if worlds.is_empty() && resources.is_empty() {
            return Err(LoadError::InvalidManifest(
                "`worlds` must list at least one world, or `resources` must list at least one kind (a data-only extension)"
                    .into(),
            ));
        }

        let mut fs = Vec::new();
        let mut process = false;
        let mut pty = false;
        let mut parsed_net: Vec<lca_permissions::NetPattern> = Vec::new();
        let mut parsed_net_local: Vec<lca_permissions::LocalPattern> = Vec::new();
        let mut net_declared = false;
        let mut parsed_oauth: Option<OAuthSettings> = None;
        let mut parsed_env: Vec<String> = Vec::new();
        let mut env_declared = false;
        let mut parsed_serve_http: Option<ServeHttpSettings> = None;
        let mut parsed_credentials = false;
        let mut parsed_completion = false;
        let mut tools = false;
        let mut parsed_ui_regions: Vec<String> = Vec::new();
        if let Some(capabilities) = value.get("capabilities") {
            let table = capabilities.as_table().ok_or_else(|| {
                LoadError::InvalidManifest("`capabilities` must be a table".into())
            })?;
            for key in table.keys() {
                match key.as_str() {
                    "fs" | "process" | "pty" | "net" | "net-local" | "oauth" | "credentials"
                    | "completion" | "ui" | "tools" | "env" | "serve-http" => {}
                    other => {
                        return Err(LoadError::InvalidManifest(format!(
                            "unknown capability `{other}`"
                        )));
                    }
                }
            }
            if let Some(cap) = table.get("fs") {
                let scopes = cap.as_table().ok_or_else(|| {
                    LoadError::InvalidManifest("`capabilities.fs` must be a table".into())
                })?;
                for (scope, mode) in scopes {
                    let mode = match mode.as_str() {
                        Some("read") => lca_permissions::FsMode::Read,
                        Some("read-write") => lca_permissions::FsMode::ReadWrite,
                        other => {
                            return Err(LoadError::InvalidManifest(format!(
                                "`{scope}` mode must be `read` or `read-write`, got {other:?}"
                            )));
                        }
                    };
                    fs.push(ScopeGrant::parse(scope, mode).map_err(|_| {
                        LoadError::InvalidManifest(format!("unknown fs scope `{scope}`"))
                    })?);
                }
                if fs.is_empty() {
                    return Err(LoadError::InvalidManifest(
                        "`capabilities.fs` must grant at least one scope".into(),
                    ));
                }
            }
            // Gh #170: the `env` grant. Exact names only; `keys = []` is a
            // declaration that grants nothing (the gh #302 precedent).
            env_declared = table.contains_key("env");
            if let Some(cap) = table.get("env") {
                let keys = cap.get("keys").and_then(|v| v.as_array()).ok_or_else(|| {
                    LoadError::InvalidManifest("`capabilities.env.keys` must be a list".into())
                })?;
                for key in keys {
                    let key = key
                        .as_str()
                        .ok_or_else(|| LoadError::InvalidManifest("env keys are strings".into()))?;
                    let key = key.trim();
                    if key.is_empty() {
                        return Err(LoadError::InvalidManifest(
                            "an env key must not be empty".into(),
                        ));
                    }
                    // A wildcard makes the consent screen say nothing: the
                    // names ARE the gate. Same rejection `net`'s bare `*` got.
                    if key.contains('*') || key.contains('?') {
                        return Err(LoadError::InvalidManifest(format!(
                            "env key `{key}` uses a wildcard; exact names only, because \
                             the names are what the user approves"
                        )));
                    }
                    // `NAME=value` in a grant would be a value nobody approved.
                    if key.contains('=') {
                        return Err(LoadError::InvalidManifest(format!(
                            "env key `{key}` carries a value; a grant names variables, \
                             never their contents"
                        )));
                    }
                    parsed_env.push(key.to_string());
                }
            }
            if let Some(cap) = table.get("process") {
                reason_of(cap, "capabilities.process")?;
                process = true;
            } // Gh #363 / ADR-0049: the inbound HTTP grant. The reason is
            // required for the same purpose it is on `process` - the consent
            // line leads with it - and the path prefix defaults to `/`, which
            // is the whole of the extension's own namespace in this phase.
            if let Some(cap) = table.get("serve-http") {
                let reason = reason_of(cap, "capabilities.serve-http")?;
                let path_prefix = match cap.get("path_prefix") {
                    Some(value) => value.as_str().ok_or_else(|| {
                        LoadError::InvalidManifest(
                            "`capabilities.serve-http.path_prefix` must be a string".into(),
                        )
                    })?,
                    None => "/",
                };
                let websocket = match cap.get("websocket") {
                    Some(value) => value.as_bool().ok_or_else(|| {
                        LoadError::InvalidManifest(
                            "`capabilities.serve-http.websocket` must be a boolean".into(),
                        )
                    })?,
                    None => false,
                };
                parsed_serve_http = Some(
                    ServeHttpSettings::parse(path_prefix, websocket)
                        .map_err(|err| LoadError::InvalidManifest(err.to_string()))?,
                );
                // The reason is required but not stored: the consent line is
                // rendered from the manifest text by `lca-registry`, which
                // needs no resolved copy here. Validating it is the point.
                let _ = reason;
            }
            if let Some(cap) = table.get("tools") {
                reason_of(cap, "capabilities.tools")?;
                tools = true;
            }
            // Gh #302: an empty `hosts` list is still a declaration.
            net_declared = table.contains_key("net") || table.contains_key("net-local");
            let mut net = Vec::new();
            if let Some(cap) = table.get("net") {
                let hosts = cap.get("hosts").and_then(|v| v.as_array()).ok_or_else(|| {
                    LoadError::InvalidManifest("`capabilities.net.hosts` must be a list".into())
                })?;
                for host in hosts {
                    let host = host.as_str().ok_or_else(|| {
                        LoadError::InvalidManifest("net host patterns are strings".into())
                    })?;
                    net.push(
                        lca_permissions::parse_net_pattern(host).map_err(|err| {
                            LoadError::InvalidManifest(format!("`{host}`: {err}"))
                        })?,
                    );
                }
            }
            let mut net_local = Vec::new();
            if let Some(cap) = table.get("net-local") {
                let addresses =
                    cap.get("addresses")
                        .and_then(|v| v.as_array())
                        .ok_or_else(|| {
                            LoadError::InvalidManifest(
                                "`capabilities.net-local.addresses` must be a list".into(),
                            )
                        })?;
                for address in addresses {
                    let address = address.as_str().ok_or_else(|| {
                        LoadError::InvalidManifest("net-local addresses are strings".into())
                    })?;
                    net_local.push(lca_permissions::parse_local_pattern(address).map_err(
                        |err| LoadError::InvalidManifest(format!("`{address}`: {err}")),
                    )?);
                }
            }
            let mut oauth = None;
            if let Some(cap) = table.get("oauth") {
                // The token exchange always needs `net` (manifest schema
                // allOf): oauth without net is a manifest error, not a
                // runtime surprise.
                if net.is_empty() {
                    return Err(LoadError::InvalidManifest(
                        "capabilities.oauth requires capabilities.net for the token exchange"
                            .into(),
                    ));
                }
                let redirect_path = cap
                    .get("redirect_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("/callback")
                    .to_string();
                let timeout_seconds = cap
                    .get("timeout_seconds")
                    .and_then(|v| v.as_integer())
                    .unwrap_or(300) as u64;
                if !(30..=600).contains(&timeout_seconds) {
                    return Err(LoadError::InvalidManifest(
                        "capabilities.oauth.timeout_seconds must be between30 and600".into(),
                    ));
                }
                oauth = Some(OAuthSettings {
                    redirect_path,
                    timeout_seconds,
                });
            }
            let mut credentials = false;
            let mut completion = false;
            if let Some(cap) = table.get("completion") {
                // The reason is required (capability catalog): consent
                // text uses it verbatim.
                reason_of(cap, "capabilities.completion")?;
                completion = true;
            }
            if let Some(cap) = table.get("credentials") {
                let namespace = cap
                    .get("namespace")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        LoadError::InvalidManifest(
                            "capabilities.credentials.namespace required".into(),
                        )
                    })?;
                // FR-PERM-6: the namespace IS the extension identity.
                if namespace != name {
                    return Err(LoadError::InvalidManifest(format!(
                        "credentials namespace `{namespace}` must equal the extension name `{name}`"
                    )));
                }
                credentials = true;
            }
            parsed_net = net;
            parsed_net_local = net_local;
            parsed_oauth = oauth;
            parsed_credentials = credentials;
            parsed_completion = completion;
            let mut ui_regions = Vec::new();
            if let Some(cap) = table.get("ui") {
                let regions = cap
                    .get("regions")
                    .and_then(|r| r.as_array())
                    .ok_or_else(|| {
                        LoadError::InvalidManifest(
                            "`capabilities.ui.regions` must be a list".into(),
                        )
                    })?;
                for region in regions {
                    let region = region.as_str().ok_or_else(|| {
                        LoadError::InvalidManifest("ui regions are strings".into())
                    })?;
                    if !matches!(region, "status-line" | "footer" | "panel" | "modal") {
                        return Err(LoadError::InvalidManifest(format!(
                            "`{region}` is not one of status-line, footer, panel, modal"
                        )));
                    }
                    if ui_regions.contains(&region.to_string()) {
                        return Err(LoadError::InvalidManifest(format!(
                            "duplicate ui region `{region}`"
                        )));
                    }
                    ui_regions.push(region.to_string());
                }
            }
            parsed_ui_regions = ui_regions;
        }

        // gh #287: `[limits]` is a resource hint, not a capability. It used
        // to be parsed inside the `[capabilities]` branch, so a limits-only
        // extension (a pure tool, hook, or workflow) silently ran on the
        // host's defaults. Parsed unconditionally now; `manifest_limits`
        // returns `Ok(None)` when the table is absent, so a manifest with
        // neither still gets no override.
        let parsed_limits = manifest_limits(&value)?;

        // Gh #157: the optional `[login]` table declares the
        // environment override the host consults before stored
        // credentials. Unknown keys stay ignored (old hosts already do
        // this with the whole table), so the key is purely additive.
        let login_env_base_url = value
            .get("login")
            .and_then(|login| login.get("env_base_url"))
            .and_then(|var| var.as_str())
            .filter(|var| !var.is_empty())
            .map(str::to_string);
        Ok(Manifest {
            name,
            version,
            abi,
            worlds,
            resources,
            fs,
            process,
            pty,
            net_declared,
            net: parsed_net,
            net_local: parsed_net_local,
            env: parsed_env,
            env_declared,
            serve_http: parsed_serve_http,
            oauth: parsed_oauth,
            credentials: parsed_credentials,
            completion: parsed_completion,
            tools,
            ui_regions: parsed_ui_regions,
            limits: parsed_limits,
            login_env_base_url,
        })
    }

    /// Whether the declared ABI line is inside the supported window
    /// (NFR-19, FR-EXT-8).
    pub fn abi_in_window(&self) -> bool {
        lca_ext_abi::abi_in_window(&self.abi)
    }
}

/// The schema's bounds for `limits` (extension-manifest.schema.json):
/// the host maximums flows.md says every request is clamped to.
pub const MAX_MEMORY_BYTES: usize = 512 * 1024 * 1024;
/// The host's per-call fuel ceiling (schema `limits.fuel_per_call`
/// maximum).
pub const MAX_FUEL_PER_CALL: u64 = 1_000_000_000;

/// Parse the manifest's optional `limits` table against the schema's
/// ranges (out-of-range is clamped at load, here we reject nonsense).
fn manifest_limits(value: &toml::Value) -> Result<Option<ExtensionLimits>, LoadError> {
    let Some(table) = value.get("limits") else {
        return Ok(None);
    };
    let memory_mb = table
        .get("memory_mb")
        .and_then(|v| v.as_integer())
        .unwrap_or(64);
    let fuel = table
        .get("fuel_per_call")
        .and_then(|v| v.as_integer())
        .unwrap_or(super::DEFAULT_FUEL_PER_CALL as i64);
    if memory_mb < 1 || fuel < 1000 {
        return Err(LoadError::InvalidManifest(
            "`limits` values are below the schema minimums".into(),
        ));
    }
    Ok(Some(ExtensionLimits {
        // gh #287: `memory_mb` is **mebibytes** (the schema's unit, and the
        // key name); `memory_bytes` and `MAX_MEMORY_BYTES` are bytes.
        // Multiply FIRST, then clamp: clamping the un-multiplied value
        // turned a 1000-MiB ask into a 1000-BYTE limiter. Saturating so a
        // wild value cannot wrap before the clamp sees it.
        memory_bytes: (memory_mb as usize)
            .saturating_mul(1024 * 1024)
            .min(MAX_MEMORY_BYTES),
        fuel_per_call: (fuel as u64).min(MAX_FUEL_PER_CALL),
        // The manifest cannot set this: the log cap is a host configuration
        // key (`extensions.log_limit_bytes`), and `ExtHost` always overrides
        // this field with its own value (see the per-instance limits merge),
        // so `0` here is never observed as "no log output".
        log_limit_bytes: 0,
    }))
}

/// Something wrong with loading (not with a call).
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// The manifest declares an ABI line outside the supported window
    /// (FR-EXT-8).
    #[error("extension targets ABI {declared}, outside this host's window {SUPPORTED_ABI_WINDOW}")]
    AbiUnsupported {
        /// The declared `major.minor`.
        declared: String,
        /// The host's window, for the report.
        window: &'static str,
    },
    /// The manifest failed validation.
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    /// Linking or instantiation failed (an approval-denied capability is
    /// absent from the import table, so this is the deny-by-default
    /// failure path from `docs/flows.md`).
    #[error("cannot link extension: {0}")]
    Link(String),
}
