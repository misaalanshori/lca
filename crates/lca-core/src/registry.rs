//! The dispatch table (SRDD crate decomposition): loaded extension
//! handles in registration order, name namespaces, and collision
//! enforcement (FR-EXT-11). Handles are `Arc<dyn ExtensionDispatch>`, so
//! nothing here branches on the delivery mode (FR-EXT-6).

use std::collections::HashMap;
use std::sync::Arc;

use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_protocol::{
    ChatMessage, CommandEffect, DispatchError, HookAction, IdentityOutcome, PostToolObservation,
    ToolCall, ToolExposure, ToolSpec, Usage,
};

/// A namespace and the tools grouped under it (gh #77's grouping:
/// related tools list under one heading with the description).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceView {
    /// The grouping name.
    pub name: String,
    /// Listed alongside the tools.
    pub description: String,
    /// Tool names in the namespace, sorted.
    pub tools: Vec<String>,
}

/// The session's dynamic tool set (pi's active tools, gh #77): `None`
/// is the default (every registered tool is active); `Some` is a
/// `set-active-tools` replacement. Only registered names take
/// effect; unknown names are ignored at set time, never here.
#[derive(Debug, Default)]
struct ActiveSet {
    names: Option<std::collections::HashSet<String>>,
    /// Bumped on every set; the turn records the transcript entry.
    revision: u64,
}

/// Built-in tool names are reserved (FR-TOOL-1's set; an extension
/// registering one is the later registration and loses, FR-EXT-11).
pub const BUILTIN_TOOLS: &[&str] = &[
    "read", "write", "edit", "list", "glob", "grep", "shell", "skill",
];

/// Pi-name aliases dispatch as built-ins (gh #119), so they are
/// reserved exactly like the canonical names: an extension registering
/// `find` loses to the host's `glob` alias the same way.
pub const BUILTIN_TOOL_ALIASES: &[&str] = &["find", "ls", "bash"];

/// The discovery tool's reserved name (gh #77): the turn serves it,
/// so no extension may register it (refused as built-in at
/// registration, like the eight built-ins).
pub const TOOL_SEARCH_NAME: &str = "tool_search";

/// Built-in slash command names, reserved the same way (SRDD's list).
pub const BUILTIN_COMMANDS: &[&str] = &["login", "logout", "usage", "model", "compact", "stats"];

/// The standard usage shape the generic `/usage` prints (ADR-0012).
fn format_usage(usage: &Usage) -> String {
    format!(
        "input {}, output {}, cache read {}, cache write {}, cost ${:.4}",
        usage.input, usage.output, usage.cache_read, usage.cache_write, usage.cost
    )
}

/// One of ADR-0012's three identity exports, which the host namespaces
/// under the extension's own name with no work by the author.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityCommand {
    /// The `login` export.
    Login,
    /// The `logout` export.
    Logout,
    /// The `usage` export.
    Usage,
}

/// Where a full command name leads: the extension's `command` world, or
/// one of its `provider`-world identity exports (FR-PROV-10).
enum Route {
    World { entry: usize, leaf: String },
    Identity { entry: usize, op: IdentityCommand },
}

/// Pending route before the entry index is known (see `register`).
enum PendingRoute {
    World(String),
    Identity(IdentityCommand),
}

/// A reported name collision (FR-EXT-11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollisionReport {
    /// The extension whose registration lost.
    pub extension: String,
    /// The colliding name.
    pub name: String,
    /// `"tool"` or `"command"`.
    pub kind: &'static str,
    /// What kept the name: `"built-in"`, an earlier extension's name, or
    /// the earlier registration's extension.
    pub winner: String,
}

struct Registered {
    handle: Arc<dyn ExtensionDispatch>,
    /// Disabled for the session (duplicate identity, FR-EXT-11).
    enabled: bool,
}

/// Every loaded extension plus its resolved names.
#[derive(Default)]
pub struct ExtensionRegistry {
    entries: Vec<Registered>,
    active: std::sync::Mutex<ActiveSet>,
    /// The running turn's nested-call server (gh #77), if any. The
    /// turn installs it around the run and clears it after; the
    /// `tools` import serves through it from blocking threads.
    nested: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<lca_ext_abi::NestedCall>>>,
    /// Bare tool name -> entry index.
    tools: HashMap<String, usize>,
    /// The kept spec per registered tool name, so arguments can be validated
    /// against the schema the model saw before `execute` runs.
    tool_schemas: HashMap<String, ToolSpec>,
    /// Full command name (`ext.command`, a claimed built-in leaf, or an
    /// auto-namespaced identity export) -> where it leads.
    commands: HashMap<String, Route>,
    collisions: Vec<CollisionReport>,
}

impl std::fmt::Debug for ExtensionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionRegistry")
            .field("entries", &self.entries.len())
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .field("commands", &self.commands.keys().collect::<Vec<_>>())
            .field("collisions", &self.collisions)
            .finish()
    }
}

impl ExtensionRegistry {
    /// An empty table.
    pub fn new() -> ExtensionRegistry {
        ExtensionRegistry::default()
    }

    /// Register one extension, resolving its names against the reserved
    /// set and everything registered earlier. A losing name is skipped
    /// and reported; a duplicate extension identity disables the later
    /// handle entirely (FR-EXT-11).
    pub fn register(&mut self, handle: Arc<dyn ExtensionDispatch>) {
        let name = handle.name().to_string();
        if self.entries.iter().any(|entry| entry.handle.name() == name) {
            self.collisions.push(CollisionReport {
                extension: name.clone(),
                name,
                kind: "extension",
                winner: "earlier registration".to_string(),
            });
            self.entries.push(Registered {
                handle,
                enabled: false,
            });
            return;
        }

        let native_claim = handle.delivery() == DeliveryMode::Native;
        let slots = handle.builtin_command_slots();
        let pending_commands = self.plan_commands(&name, handle.as_ref(), native_claim, &slots);
        let pending_tools = self.plan_tools(&name, handle.as_ref());

        let entry_index = self.entries.len();
        for spec in pending_tools {
            self.tools.insert(spec.name.clone(), entry_index);
            self.tool_schemas.insert(spec.name.clone(), spec);
        }
        for (full, pending) in pending_commands {
            let route = match pending {
                PendingRoute::World(leaf) => Route::World {
                    entry: entry_index,
                    leaf,
                },
                PendingRoute::Identity(op) => Route::Identity {
                    entry: entry_index,
                    op,
                },
            };
            self.commands.insert(full, route);
        }
        self.entries.push(Registered {
            handle,
            enabled: true,
        });
    }

    /// Plan one handle's command routes: reserved built-in slots, the
    /// provider identity trio, and collisions (ADR-0019, FR-PROV-10).
    fn plan_commands(
        &mut self,
        name: &str,
        handle: &dyn ExtensionDispatch,
        native_claim: bool,
        slots: &[String],
    ) -> Vec<(String, PendingRoute)> {
        let mut pending: Vec<(String, PendingRoute)> = Vec::new();
        if handle.worlds().contains(&World::Command) {
            match handle.command_specs() {
                Ok(specs) => {
                    for spec in specs {
                        // Only a native first-party handle may claim a
                        // reserved built-in slot, and only for one of the
                        // documented built-in names (ADR-0019); everything
                        // else namespaces as `<extension>.<command>`
                        // (ADR-0012's mechanism), so a third-party
                        // command can never shadow a built-in.
                        let claims_builtin = native_claim
                            && slots.contains(&spec.name)
                            && BUILTIN_COMMANDS.contains(&spec.name.as_str());
                        let full = if claims_builtin {
                            spec.name.clone()
                        } else {
                            format!("{name}.{}", spec.name)
                        };
                        let winner = self
                            .commands
                            .get(&full)
                            .map(|route| self.route_name(route).to_string());
                        match winner {
                            Some(winner) => self.collisions.push(CollisionReport {
                                extension: name.to_string(),
                                name: full,
                                kind: "command",
                                winner,
                            }),
                            None => pending.push((full, PendingRoute::World(spec.name))),
                        }
                    }
                }
                Err(err) => {
                    // A handle that cannot enumerate its commands keeps its
                    // other worlds; report through the collision log.
                    self.collisions.push(CollisionReport {
                        extension: name.to_string(),
                        name: "(command world)".to_string(),
                        kind: "command",
                        winner: format!("unavailable: {err}"),
                    });
                }
            }
        }

        if handle.worlds().contains(&World::Provider) {
            // FR-PROV-10 / ADR-0012: the host namespaces the three
            // identity exports itself, so two providers that both export
            // `usage` can never collide.
            for (leaf, op) in [
                ("login", IdentityCommand::Login),
                ("logout", IdentityCommand::Logout),
                ("usage", IdentityCommand::Usage),
            ] {
                let full = format!("{name}.{leaf}");
                let winner = self
                    .commands
                    .get(&full)
                    .map(|route| self.route_name(route).to_string());
                match winner {
                    Some(winner) => self.collisions.push(CollisionReport {
                        extension: name.to_string(),
                        name: full,
                        kind: "command",
                        winner,
                    }),
                    None => pending.push((full, PendingRoute::Identity(op))),
                }
            }
        }
        pending
    }

    /// Plan one handle's tool specs, refusing built-in shadowing and
    /// earlier-registered names. Handles merge their own worlds inside
    /// `tool_specs` (a suite guest serves every tool through its
    /// catalog, gh #77); the registry resolves names, not worlds.
    fn plan_tools(&mut self, name: &str, handle: &dyn ExtensionDispatch) -> Vec<ToolSpec> {
        let mut pending: Vec<ToolSpec> = Vec::new();
        if handle.worlds().contains(&World::Tool) || handle.worlds().contains(&World::ToolCatalog) {
            match handle.tool_specs() {
                Ok(specs) => {
                    for spec in specs {
                        // `tool_search` is served by the turn itself (gh
                        // #77's discovery): no extension may claim it.
                        let winner = if BUILTIN_TOOLS.contains(&spec.name.as_str())
                            || BUILTIN_TOOL_ALIASES.contains(&spec.name.as_str())
                            || spec.name == TOOL_SEARCH_NAME
                        {
                            Some("built-in".to_string())
                        } else {
                            self.tools
                                .get(&spec.name)
                                .map(|index| self.entry_name(*index).to_string())
                        };
                        match winner {
                            Some(winner) => self.collisions.push(CollisionReport {
                                extension: name.to_string(),
                                name: spec.name,
                                kind: "tool",
                                winner,
                            }),
                            None => pending.push(spec),
                        }
                    }
                }
                Err(err) => {
                    self.collisions.push(CollisionReport {
                        extension: name.to_string(),
                        name: "(tool world)".to_string(),
                        kind: "tool",
                        winner: format!("unavailable: {err}"),
                    });
                }
            }
        }
        pending
    }

    fn entry_name(&self, index: usize) -> &str {
        self.entries[index].handle.name()
    }

    fn route_name(&self, route: &Route) -> &str {
        match route {
            Route::World { entry, .. } | Route::Identity { entry, .. } => self.entry_name(*entry),
        }
    }

    /// Every enabled provider extension's name, in registration order
    /// (FR-PROV-11's list for the generic `/login`).
    pub fn provider_names(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| entry.enabled && entry.handle.worlds().contains(&World::Provider))
            .map(|entry| entry.handle.name().to_string())
            .collect()
    }

    /// Every enabled extension handle, in registration order (gh #12:
    /// the host collects optional per-handle surfaces, like the
    /// Rust-side markdown transform, from these). Disabled entries
    /// (collision losers, FR-EXT-11) contribute nothing.
    pub fn handles(&self) -> Vec<Arc<dyn ExtensionDispatch>> {
        self.entries
            .iter()
            .filter(|entry| entry.enabled)
            .map(|entry| entry.handle.clone())
            .collect()
    }

    /// The enabled provider registered under `name` (the configured
    /// active provider resolves through this; `None` is FR-PROV-6's
    /// data and a valid zero-provider state, FR-PROV-9).
    pub fn provider(&self, name: &str) -> Option<&Arc<dyn ExtensionDispatch>> {
        self.entries
            .iter()
            .find(|entry| {
                entry.enabled
                    && entry.handle.name() == name
                    && entry.handle.worlds().contains(&World::Provider)
            })
            .map(|entry| &entry.handle)
    }

    /// Enable or disable one registered extension for the session
    /// (FR-PROV-9's disable knob; enablement itself lives in the grant
    /// store, per `docs/configuration.md`).
    pub fn set_enabled(&mut self, name: &str, enabled: bool) {
        for entry in &mut self.entries {
            if entry.handle.name() == name {
                entry.enabled = enabled;
            }
        }
    }

    /// Run one identity future to completion from the input editor's
    /// thread. That thread is driven by `main`'s runtime, where a
    /// nested `block_on` panics - the old comment claiming command
    /// invocation ran outside any async context was wrong, and the
    /// audit's test inside a live runtime caught it; `drive_blocking`
    /// owns the bridge (its own thread, its own runtime, joined).
    fn drive<T: Send + 'static>(
        &self,
        future: impl std::future::Future<Output = T> + Send + 'static,
    ) -> T {
        crate::drive_blocking(future)
    }

    /// One identity operation rendered as a notice (the effect the
    /// input editor shows). Text shape is deterministic for tests.
    fn identity_effect(
        &self,
        handle: &Arc<dyn ExtensionDispatch>,
        op: IdentityCommand,
    ) -> CommandEffect {
        let handle = handle.clone();
        self.drive(async move {
            let name = handle.name().to_string();
            match op {
                IdentityCommand::Login => match handle.identity_login().await {
                    Ok(IdentityOutcome::Ok) => {
                        CommandEffect::ShowWidget(format!("logged in via `{name}`"))
                    }
                    Ok(IdentityOutcome::NotSupported) => {
                        CommandEffect::ShowWidget(format!("login is not supported by `{name}`"))
                    }
                    Ok(IdentityOutcome::Failed(reason)) => {
                        CommandEffect::ShowWidget(format!("login failed: {reason}"))
                    }
                    Err(err) => CommandEffect::ShowWidget(format!("login failed: {err}")),
                },
                IdentityCommand::Logout => match handle.identity_logout().await {
                    Ok(IdentityOutcome::Ok) => {
                        CommandEffect::ShowWidget(format!("logged out of `{name}`"))
                    }
                    Ok(IdentityOutcome::NotSupported) => {
                        CommandEffect::ShowWidget(format!("logout is not supported by `{name}`"))
                    }
                    Ok(IdentityOutcome::Failed(reason)) => {
                        CommandEffect::ShowWidget(format!("logout failed: {reason}"))
                    }
                    Err(err) => CommandEffect::ShowWidget(format!("logout failed: {err}")),
                },
                IdentityCommand::Usage => match handle.identity_usage().await {
                    Ok(Ok(usage)) => CommandEffect::ShowWidget(format_usage(&usage)),
                    Ok(Err(IdentityOutcome::NotSupported)) => {
                        CommandEffect::ShowWidget(format!("usage is not supported by `{name}`"))
                    }
                    Ok(Err(IdentityOutcome::Failed(reason))) => {
                        CommandEffect::ShowWidget(format!("usage failed: {reason}"))
                    }
                    Ok(Err(IdentityOutcome::Ok)) => {
                        CommandEffect::ShowWidget(format!("`{name}` returned no usage record"))
                    }
                    Err(err) => CommandEffect::ShowWidget(format!("usage failed: {err}")),
                },
            }
        })
    }

    /// The generic `/login`, `/logout`, and `/usage` (FR-PROV-11):
    /// `login` lists every installed provider when the user has not
    /// chosen one, `logout` and `usage` follow `active`. `None` means
    /// zero providers are enabled — a valid state (FR-PROV-9) that the
    /// caller reports as FR-PROV-6's "no model is available".
    pub fn invoke_generic(
        &self,
        command: &str,
        argument: &str,
        active: &str,
    ) -> Option<CommandEffect> {
        let names = self.provider_names();
        if names.is_empty() {
            return None;
        }
        let op = match command {
            "login" => IdentityCommand::Login,
            "logout" => IdentityCommand::Logout,
            "usage" => IdentityCommand::Usage,
            _ => return None,
        };
        let target = match op {
            IdentityCommand::Login => {
                if !argument.is_empty() {
                    if names.iter().any(|name| name == argument) {
                        argument.to_string()
                    } else {
                        return Some(CommandEffect::ShowWidget(format!(
                            "no provider named `{argument}`; installed: {}",
                            names.join(", ")
                        )));
                    }
                } else if names.len() == 1 {
                    names[0].clone()
                } else {
                    return Some(CommandEffect::ShowWidget(format!(
                        "{} installed: {}. Choose: /login <name>",
                        names.len(),
                        names.join(", ")
                    )));
                }
            }
            IdentityCommand::Logout | IdentityCommand::Usage => match self.provider(active) {
                Some(_) => active.to_string(),
                None => {
                    return Some(CommandEffect::ShowWidget(format!(
                        "no provider is enabled (configured `{active}` is unavailable);                          installed: {}",
                        names.join(", ")
                    )));
                }
            },
        };
        let handle = self.provider(&target)?.clone();
        Some(self.identity_effect(&handle, op))
    }

    /// Every registered handle's name, enabled or not (the grant
    /// store's enablement filter runs against this, FR-PROV-9).
    pub fn registered_names(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| entry.handle.name().to_string())
            .collect()
    }

    /// Every collision reported during registration (FR-EXT-11's report).
    pub fn collisions(&self) -> &[CollisionReport] {
        &self.collisions
    }

    /// Enabled handles in registration order.
    pub fn enabled(&self) -> impl Iterator<Item = &Arc<dyn ExtensionDispatch>> {
        self.entries
            .iter()
            .filter(|entry| entry.enabled)
            .map(|entry| &entry.handle)
    }

    /// Whether this handle is enabled (FR-EXT-7's list data pairs it
    /// with `delivery()`).
    pub fn is_enabled(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.handle.name() == name && entry.enabled)
    }

    /// The extension that registered a bare tool name, if any.
    pub fn tool_owner(&self, tool: &str) -> Option<&Arc<dyn ExtensionDispatch>> {
        self.tools
            .get(tool)
            .map(|index| &self.entries[*index].handle)
    }

    /// The schema of a registered tool, for validating its arguments before
    /// dispatch (extension authoring guide: the host validates, then calls).
    pub fn tool_schema(&self, name: &str) -> Option<&ToolSpec> {
        self.tool_schemas.get(name)
    }

    /// Whether a tool name is active (gh #77): the default set holds
    /// every registered non-hidden tool; a `set-active-tools`
    /// replacement holds exactly its applied names.
    fn is_active(&self, name: &str, exposure: ToolExposure) -> bool {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        match &active.names {
            None => exposure != ToolExposure::Hidden,
            Some(names) => names.contains(name),
        }
    }

    /// Replace the session's active tool set (pi's `setActiveTools`,
    /// gh #77): only registered names take effect; unknown names are
    /// ignored and reported, never applied. Returns `(applied,
    /// ignored)`, both sorted. The turn records the transcript entry
    /// before the next model request.
    pub fn set_active_tools(&self, names: &[String]) -> (Vec<String>, Vec<String>) {
        let mut applied = Vec::new();
        let mut ignored = Vec::new();
        for name in names {
            if self.tool_schemas.contains_key(name) {
                applied.push(name.clone());
            } else {
                ignored.push(name.clone());
            }
        }
        applied.sort();
        ignored.sort();
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        active.names = Some(applied.iter().cloned().collect());
        active.revision += 1;
        (applied, ignored)
    }

    /// The active tool names, sorted (gh #77's `getActiveTools`).
    pub fn active_tools(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tool_schemas
            .iter()
            .filter(|(name, spec)| self.is_active(name, spec.exposure))
            .map(|(name, _)| name.clone())
            .collect();
        names.sort();
        names
    }

    /// The active set's revision: the turn compares it across requests
    /// to record transcript entries for mid-turn changes (gh #77).
    pub fn active_revision(&self) -> u64 {
        self.active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .revision
    }

    /// Whether the host may run a tool by name right now (gh #77's
    /// callable set): direct tools while active, codemode and deferred
    /// tools whenever registered; model-only and hidden tools never.
    pub fn is_callable(&self, name: &str) -> bool {
        let enabled = self
            .tools
            .get(name)
            .map(|index| self.entries[*index].enabled)
            .unwrap_or(false);
        if !enabled {
            return false;
        }
        match self.tool_schemas.get(name) {
            Some(spec) => match spec.exposure {
                ToolExposure::Direct => self.is_active(name, spec.exposure),
                ToolExposure::Codemode | ToolExposure::Deferred => true,
                ToolExposure::ModelOnly | ToolExposure::Hidden => false,
            },
            None => false,
        }
    }

    /// The callable tools for orchestrators (the `tools` import's
    /// `list-tools`, gh #77): name plus description, sorted.
    pub fn callable_names(&self) -> Vec<(String, String)> {
        let mut names: Vec<(String, String)> = self
            .tool_schemas
            .iter()
            .filter(|(name, _)| self.is_callable(name))
            .map(|(_, spec)| (spec.name.clone(), spec.description.clone()))
            .collect();
        names.sort();
        names
    }

    /// Deferred discovery (gh #77's `tool_search` shape, pi's
    /// `deferred`): codemode and deferred tools whose name,
    /// description, or namespace matches the query, sorted. Hidden
    /// and model-only tools never match; direct tools are declared,
    /// not discovered. Matching is case-insensitive substring; an
    /// empty query lists every discoverable tool.
    pub fn tool_search(&self, query: &str) -> Vec<ToolSpec> {
        let needle = query.to_lowercase();
        let mut matches: Vec<ToolSpec> = self
            .tool_specs()
            .into_iter()
            .filter(|spec| {
                matches!(
                    spec.exposure,
                    ToolExposure::Codemode | ToolExposure::Deferred
                )
            })
            .filter(|spec| {
                if needle.is_empty() {
                    return true;
                }
                let haystack = match &spec.namespace {
                    Some(namespace) => format!(
                        "{} {} {} {}",
                        spec.name, spec.description, namespace.name, namespace.description
                    ),
                    None => format!("{} {}", spec.name, spec.description),
                };
                haystack.to_lowercase().contains(&needle)
            })
            .collect();
        matches.sort_by(|a, b| a.name.cmp(&b.name));
        matches
    }

    /// Namespace grouping rows (gh #77): one row per namespace with
    /// its enabled tools, both sorted. Tools without a namespace
    /// group nowhere.
    pub fn namespaces(&self) -> Vec<NamespaceView> {
        let mut groups: HashMap<String, NamespaceView> = HashMap::new();
        for spec in self.tool_specs() {
            if let Some(namespace) = &spec.namespace {
                groups
                    .entry(namespace.name.clone())
                    .or_insert_with(|| NamespaceView {
                        name: namespace.name.clone(),
                        description: namespace.description.clone(),
                        tools: Vec::new(),
                    })
                    .tools
                    .push(spec.name.clone());
            }
        }
        let mut views: Vec<NamespaceView> = groups.into_values().collect();
        for view in &mut views {
            view.tools.sort();
        }
        views.sort_by(|a, b| a.name.cmp(&b.name));
        views
    }

    /// What the next declaration request carries (gh #77): enabled,
    /// active, `direct` tools, sorted. Everything else reaches the
    /// model through discovery, orchestration, or not at all.
    pub fn declared_tool_specs(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self
            .tool_specs()
            .into_iter()
            .filter(|spec| {
                spec.exposure == ToolExposure::Direct && self.is_active(&spec.name, spec.exposure)
            })
            .collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }

    /// Every extension tool spec, for the provider's tool list. Sorted by
    /// name: a provider's prompt cache and the request-assembly snapshot key
    /// on stable bytes, and the registry's maps are unordered.
    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self
            .tool_schemas
            .iter()
            .filter(|(name, _)| {
                self.tools
                    .get(*name)
                    .map(|index| self.entries[*index].enabled)
                    .unwrap_or(false)
            })
            .map(|(_, spec)| spec.clone())
            .collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }

    /// Full command names without the leading slash (the input editor
    /// adds it).
    pub fn command_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.commands.keys().cloned().collect();
        names.sort();
        names
    }

    /// Invoke a full command name (the host namespaced it, it is a
    /// claimed built-in slot, or it is an auto-namespaced identity
    /// export, FR-PROV-10).
    pub fn invoke_command(&self, full: &str, argument: &str) -> Option<CommandEffect> {
        let route = self.commands.get(full)?;
        let (entry, handle) = match route {
            Route::World { entry, .. } | Route::Identity { entry, .. } => {
                (*entry, &self.entries[*entry].handle)
            }
        };
        if !self.entries[entry].enabled {
            return None;
        }
        match route {
            Route::World { leaf, .. } => handle.invoke_command(leaf, argument).ok(),
            Route::Identity { op, .. } => Some(self.identity_effect(handle, *op)),
        }
    }

    /// The first enabled `compaction` strategy (FR-SESS-5): compaction
    /// only ever happens through this world, and only one extension
    /// owns a given turn's compact.
    pub fn compaction_strategy(&self) -> Option<&Arc<dyn ExtensionDispatch>> {
        self.entries
            .iter()
            .find(|entry| entry.enabled && entry.handle.worlds().contains(&World::Compaction))
            .map(|entry| &entry.handle)
    }

    /// Apply every enabled `context-transform` extension in
    /// installation order (FR-CTX-2), each output feeding the next. The
    /// first rejection - or host-level failure, which must not let
    /// unreviewed messages through either - stops the chain
    /// (FR-CTX-3).
    pub async fn transform(
        &self,
        mut messages: Vec<ChatMessage>,
    ) -> Result<Vec<ChatMessage>, String> {
        for handle in self
            .enabled()
            .filter(|handle| handle.worlds().contains(&World::ContextTransform))
        {
            // The clone keeps the input alive for a Disabled skip;
            // every other arm replaces or returns it anyway.
            match handle.transform_messages(messages.clone()).await {
                Ok(Ok(list)) => messages = list,
                Ok(Err(reason)) => return Err(reason),
                // A handle that just disabled itself (trapped, hit its
                // limits) drops out of the chain the same way the hooks
                // loops skip it: FR-EXT-5's host-continues rule outranks
                // FR-CTX-2 for a dead extension. Every other failure
                // stops the turn - messages that half-transformed must
                // not reach the provider.
                Err(DispatchError::Disabled) => continue,
                Err(err) => {
                    return Err(format!(
                        "the context transform `{}` failed: {err}",
                        handle.name()
                    ));
                }
            }
        }
        Ok(messages)
    }

    /// Merge the `pre-turn` hook across every enabled extension
    /// (`post-turn` and close hooks are the same loop with other
    /// methods). Errors disable inside the host (FR-EXT-3) and are
    /// skipped here so one broken extension cannot end a turn.
    pub async fn on_pre_turn(&self) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            let _ = handle.on_pre_turn().await;
        }
    }

    /// Run the `pre-tool-use` hook across enabled extensions in
    /// registration order: the first deny wins, a replace stops the
    /// chain (the replacement is not re-hooked, SRDD hooks), and hook
    /// errors are reported-and-skipped (FR-EXT-3).
    pub async fn pre_tool_use(
        &self,
        call: &ToolCall,
        on_error: &mut dyn FnMut(&Arc<dyn ExtensionDispatch>, &DispatchError),
    ) -> HookAction {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            match handle.on_pre_tool_use(call).await {
                Ok(HookAction::Allow) => {}
                Ok(action) => return action,
                Err(err) => on_error(handle, &err),
            }
        }
        HookAction::Allow
    }

    /// `post-tool-use` across enabled hook extensions (FR-CORE-10's
    /// observe side).
    pub async fn on_post_tool_use(&self, call: &ToolCall, result: &lca_protocol::ToolResult) {
        let observation = PostToolObservation {
            call: call.clone(),
            result: result.clone(),
        };
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            let _ = handle.on_post_tool_use(&observation).await;
        }
    }

    /// `post-turn-end` with `ok` or `error`.
    pub async fn on_post_turn_end(&self, status: &str) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            let _ = handle.on_post_turn_end(status).await;
        }
    }

    /// `attention-required`: the turn failed and the user should look. The
    /// reason is the same text the interface surfaced.
    pub async fn on_attention_required(&self, reason: &str) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            let _ = handle.on_attention_required(reason).await;
        }
    }

    /// Install the running turn's nested-call server (gh #77).
    pub fn install_nested(&self, tx: tokio::sync::mpsc::UnboundedSender<lca_ext_abi::NestedCall>) {
        *self
            .nested
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(tx);
    }

    /// Remove the turn's nested-call server.
    pub fn clear_nested(&self) {
        *self
            .nested
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
    }

    /// The installed server, if a turn is running.
    pub fn nested_slot(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedSender<lca_ext_abi::NestedCall>> {
        self.nested
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// The `tools` import's registry surface (gh #77): the host holds
    /// the registry behind the trait so the crates stay decoupled.
    pub fn as_tools_view(
        self: &std::sync::Arc<Self>,
    ) -> std::sync::Arc<dyn lca_ext_abi::ToolsRegistryView> {
        self.clone()
    }

    /// Force any running WASM call to trap (epoch interruption,
    /// FR-CONC-1); native handles share the caller's cancellation flag.
    pub fn interrupt_all(&self) {
        for handle in self.enabled() {
            handle.interrupt();
        }
    }

    /// Composable `tool_call` mutation (gh #45): every enabled
    /// `hooks-tool-call` extension runs in order, each seeing the
    /// previous handler's arguments. A `block` veto ends the chain
    /// with the reason; hook errors are reported-and-skipped
    /// (FR-EXT-3), never silent vetoes.
    pub async fn mutate_tool_call(&self, call: &ToolCall) -> Result<ToolCall, String> {
        let mut current = call.clone();
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksToolCall))
        {
            match handle.on_tool_call(&current).await {
                Ok(patch) => {
                    if let Some(reason) = patch.block {
                        return Err(reason);
                    }
                    if let Some(arguments) = patch.arguments {
                        current.arguments = arguments;
                    }
                }
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "tool_call hook failed; skipped");
                }
            }
        }
        Ok(current)
    }

    /// Composable `tool_result` mutation (gh #45): every enabled
    /// `hooks-tool-result` extension runs in order over the same
    /// result; omitted fields stay. Errors skip (FR-EXT-3).
    pub async fn compose_tool_result(
        &self,
        call: &ToolCall,
        result: lca_protocol::ToolResult,
    ) -> lca_protocol::ToolResult {
        let mut current = result;
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksToolResult))
        {
            match handle.on_tool_result(call, &current).await {
                Ok(patch) => {
                    if let Some(content) = patch.content {
                        current.content = content;
                    }
                    if let Some(is_error) = patch.is_error {
                        current.status = if is_error {
                            lca_protocol::ToolResultStatus::Error
                        } else {
                            lca_protocol::ToolResultStatus::Ok
                        };
                    }
                }
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "tool_result hook failed; skipped");
                }
            }
        }
        current
    }

    /// `message_end` replacement text (gh #45): every enabled
    /// `hooks-message` extension sees the current text; the last
    /// replacement wins. `None` observes. Errors skip (FR-EXT-3).
    pub async fn message_end_replacement(&self, role: &str, text: &str) -> Option<String> {
        let mut current = text.to_string();
        let mut replaced = false;
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksMessage))
        {
            match handle.on_message_end(role, &current).await {
                Ok(Some(replacement)) => {
                    current = replacement;
                    replaced = true;
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "message_end hook failed; skipped");
                }
            }
        }
        replaced.then_some(current)
    }

    /// Fold one handler's append into the composed decision (gh #45):
    /// non-empty appends concatenate with a blank line.
    pub(crate) fn merge_append(
        decision: &mut lca_protocol::SettleDecision,
        append: Option<String>,
    ) {
        if let Some(append) = append.filter(|text| !text.is_empty()) {
            match &mut decision.append {
                Some(existing) => {
                    existing.push_str("\n\n");
                    existing.push_str(&append);
                }
                None => decision.append = Some(append),
            }
        }
    }

    /// Actionable settle (gh #45): consult one phase (`turn_end`,
    /// then `agent_before_settle`). Appends concatenate; any
    /// `continue-once` continues. Errors settle (FR-EXT-3).
    pub async fn settle_phase(
        &self,
        before_settle: bool,
        rounds: u32,
        tool_calls: u32,
        status: &str,
    ) -> lca_protocol::SettleDecision {
        let mut decision = lca_protocol::SettleDecision::default();
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksSettle))
        {
            let outcome = if before_settle {
                handle
                    .on_agent_before_settle(rounds, tool_calls, status)
                    .await
            } else {
                handle.on_turn_end(rounds, tool_calls, status).await
            };
            match outcome {
                Ok(patch) => {
                    Self::merge_append(&mut decision, patch.append);
                    decision.continue_once |= patch.continue_once;
                }
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "settle hook failed; settling");
                }
            }
        }
        decision
    }

    /// `session_before_compact` veto (gh #45): the first deny
    /// cancels the compaction with its reason. Errors allow
    /// (FR-EXT-3: a broken watcher must not freeze the session).
    pub async fn session_before_compact(&self, reason: &str) -> Result<(), String> {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksCompaction))
        {
            match handle.on_session_before_compact(reason).await {
                Ok(lca_protocol::CompactVerdict::Allow) => {}
                Ok(lca_protocol::CompactVerdict::Deny(reason)) => return Err(reason),
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "before-compact hook failed; allowing");
                }
            }
        }
        Ok(())
    }

    /// `session_compact_failed` observation (gh #45).
    pub async fn session_compact_failed(&self, reason: &str, error: Option<&str>) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksCompaction))
        {
            let _ = handle.on_session_compact_failed(reason, error).await;
        }
    }

    /// `cache_warming_decision` votes (gh #45): every enabled
    /// `hooks-cache` extension votes; any `false` skips warming.
    /// Errors warm (FR-EXT-3).
    pub async fn cache_warm(&self, provider: &str, model: &str) -> bool {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksCache))
        {
            match handle.on_cache_warming_decision(provider, model).await {
                Ok(true) => {}
                Ok(false) => return false,
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "cache hook failed; warming");
                }
            }
        }
        true
    }

    /// `project_trust` votes (gh #45): the first yes/no decides;
    /// undecided falls through. Returns the vote and whether to
    /// remember it. Errors are undecided (FR-EXT-3).
    pub async fn project_trust(&self, cwd: &str) -> (lca_protocol::TrustVote, bool) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksTrust))
        {
            match handle.on_project_trust(cwd).await {
                Ok((lca_protocol::TrustVote::Undecided, _)) => {}
                Ok((vote, remember)) => return (vote, remember),
                Err(err) => {
                    tracing::warn!(extension = handle.name(), %err, "trust hook failed; undecided");
                }
            }
        }
        (lca_protocol::TrustVote::Undecided, false)
    }

    /// `provider_stream_event` observation (gh #45): normalized
    /// events, in order, after the stream closes. Errors skip.
    pub async fn observe_stream_event(&self, provider: &str, model: &str, kind: &str, data: &str) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::HooksStream))
        {
            let _ = handle.on_stream_event(provider, model, kind, data).await;
        }
    }

    /// `session-close`, called when a session ends.
    pub async fn on_session_close(&self) {
        for handle in self
            .enabled()
            .filter(|h| h.worlds().contains(&World::Hooks))
        {
            let _ = handle.on_session_close().await;
        }
    }
}

impl lca_ext_abi::ToolsRegistryView for ExtensionRegistry {
    fn callable_names(&self) -> Vec<(String, String)> {
        ExtensionRegistry::callable_names(self)
    }

    fn is_callable(&self, name: &str) -> bool {
        ExtensionRegistry::is_callable(self, name)
    }

    fn active_tools(&self) -> Vec<String> {
        ExtensionRegistry::active_tools(self)
    }

    fn set_active_tools(&self, names: &[String]) -> (Vec<String>, Vec<String>) {
        ExtensionRegistry::set_active_tools(self, names)
    }

    fn install_nested(&self, tx: tokio::sync::mpsc::UnboundedSender<lca_ext_abi::NestedCall>) {
        ExtensionRegistry::install_nested(self, tx)
    }

    fn clear_nested(&self) {
        ExtensionRegistry::clear_nested(self)
    }

    fn nested_slot(&self) -> Option<tokio::sync::mpsc::UnboundedSender<lca_ext_abi::NestedCall>> {
        ExtensionRegistry::nested_slot(self)
    }
}
