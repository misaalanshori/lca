//! The dispatch table (SRDD crate decomposition): loaded extension
//! handles in registration order, name namespaces, and collision
//! enforcement (FR-EXT-11). Handles are `Arc<dyn ExtensionDispatch>`, so
//! nothing here branches on the delivery mode (FR-EXT-6).

use crate::active_set::ActiveSet;
use std::collections::HashMap;
use std::sync::Arc;

use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_protocol::{
    ChatMessage, CommandEffect, DispatchError, HookAction, IdentityOutcome, PostToolObservation,
    ToolAnnotations, ToolCall, ToolExposure, ToolSpec,
};

/// The name tables plus the planning rules (gh #584), split for the file
/// ceiling. One `Table`, one set of rules, shared by registration and re-plan.
mod plan;
use plan::Route;

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
    /// Atomic: the TUI manager flips it live through the shared
    /// registry (gh #3) while turns hold the same `Arc`.
    enabled: std::sync::atomic::AtomicBool,
}

impl Registered {
    /// Session-enabled, read atomically (gh #3's live toggle).
    fn is_enabled(&self) -> bool {
        self.enabled.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Every loaded extension plus its resolved names.
#[derive(Default)]
pub struct ExtensionRegistry {
    entries: Vec<Registered>,
    /// The preferred compaction strategy (gh #3): an extension name
    /// the manager picked. Empty means automatic (first enabled).
    preferred_compaction: std::sync::Mutex<Option<String>>,
    pub(crate) active: std::sync::Mutex<ActiveSet>,
    /// The running turn's nested-call server (gh #77), if any. The
    /// turn installs it around the run and clears it after; the
    /// `tools` import serves through it from blocking threads.
    nested: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<lca_ext_abi::NestedCall>>>,
    /// Native capability engines the host resets at the turn boundary
    /// (gh #97): extension name -> weak engine handle. Noting an engine
    /// is registration bookkeeping — the host created the engine and
    /// hands it to the native constructor — never an implementor
    /// burden, which is what makes an interrupt-only native safe. Weak
    /// so a rebuilt registry never keeps a dead engine alive.
    engines: std::sync::Mutex<HashMap<String, std::sync::Weak<lca_tools::Capabilities>>>,
    /// The name tables (gh #584): tools, kept specs, commands, and the
    /// collision log. One lock, because a re-plan must install a whole
    /// handle's names as one snapshot - a reader must never see half of
    /// one handle's set.
    pub(crate) table: std::sync::Mutex<plan::Table>,
}

impl std::fmt::Debug for ExtensionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let table = self.lock_table();
        f.debug_struct("ExtensionRegistry")
            .field("entries", &self.entries.len())
            .field("tools", &table.tools.keys().collect::<Vec<_>>())
            .field("commands", &table.commands.keys().collect::<Vec<_>>())
            .field("collisions", &table.collisions)
            .field("engines", &crate::lock(&self.engines).len())
            .finish()
    }
}

/// Manager support (gh #3): the compaction preference and the row
/// listing live beside the table so this file stays under the
/// ceiling.
#[path = "registry_manage.rs"]
mod manage;

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
            self.table
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .insert_collision(CollisionReport {
                    extension: name.clone(),
                    name,
                    kind: "extension",
                    winner: "earlier registration".to_string(),
                });
            self.entries.push(Registered {
                handle,
                enabled: std::sync::atomic::AtomicBool::new(false),
            });
            return;
        }

        let worlds = handle.worlds();
        let native_claim = handle.delivery() == DeliveryMode::Native;
        let slots = handle.builtin_command_slots();
        // Gh #584: the fetch is the handle's own work (a WASM handle runs one
        // blocking component call, and a guest may call back into the host from
        // inside it), so it happens with no lock held; the install is a pure
        // function of what came back and the table as it stands.
        let fetched = plan::fetch(handle.as_ref(), &worlds);
        let entry_index = self.entries.len();
        let planned = {
            let table = self
                .table
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            plan::plan(
                &table,
                &self.entries,
                &name,
                &worlds,
                native_claim,
                &slots,
                fetched,
            )
        };
        {
            let mut table = self
                .table
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            table.insert_collisions(planned.reports.clone());
            table.install(entry_index, planned);
        }
        self.entries.push(Registered {
            handle,
            enabled: std::sync::atomic::AtomicBool::new(true),
        });
    }

    /// Re-plan every handle whose **specs version moved** (gh #584).
    ///
    /// This is what closes the landing gap: an MCP server that connects after
    /// `assemble` registered the bridge bumps its version on `land`, and the
    /// next declaration re-reads that handle's live specs, so the model is
    /// told about the tool and `tool_owner` can find it.
    ///
    /// The cost when nothing changed is one atomic load per handle: a handle
    /// answering the trait's `None` default (every WASM guest, every in-tree
    /// extension) is never re-planned, and a settled MCP bridge is compared
    /// against the version it was last planned from. So two consecutive reads
    /// of a settled registry return byte-identical vectors - a re-read that
    /// reordered or re-rendered anything would bust the prompt cache every
    /// turn, which is the regression this shape exists to avoid.
    ///
    /// Failures are reported, never fatal: a handle that cannot answer keeps
    /// the names it has (the re-plan's `(tool world)` collision says so).
    pub(crate) fn refresh(&self) {
        let names: Vec<String> = self
            .entries
            .iter()
            .map(|entry| entry.handle.name().to_string())
            .collect();
        for (index, entry) in self.entries.iter().enumerate() {
            let Some(version) = entry.handle.specs_version() else {
                continue;
            };
            // The watermark is checked, and the swap performed, under **one**
            // table lock (gh #584): `withdraw` then `install` in two scopes would
            // let a concurrent reader see the entry's names missing between them,
            // i.e. a tool that exists flickering out of the declaration. The
            // fetch happens before this, so no foreign call runs under the lock.
            let table = self.lock_table();
            if table.planned.get(index).copied() == Some(version) {
                continue;
            }
            drop(table);
            let worlds = entry.handle.worlds();
            let native_claim = entry.handle.delivery() == DeliveryMode::Native;
            let slots = entry.handle.builtin_command_slots();
            let fetched = plan::fetch(entry.handle.as_ref(), &worlds);
            let mut table = self.lock_table();
            // Re-check: another reader may have re-planned this entry while the
            // handle was being asked. Both plans are the same, but skipping the
            // second one keeps this from doing the work twice.
            if table.planned.get(index).copied() == Some(version) {
                continue;
            }
            table.withdraw(index);
            let planned = plan::plan(
                &table,
                &self.entries,
                &names[index],
                &worlds,
                native_claim,
                &slots,
                fetched,
            );
            table.insert_collisions(planned.reports.clone());
            table.install(index, planned);
            // Written with the install, so a reader either sees the new names
            // and the new version, or neither.
            table.note_planned(index, version);
        }
    }

    /// The table lock (poisoning recovers: a dead manager thread must not
    /// wedge a turn).
    pub(crate) fn lock_table(&self) -> std::sync::MutexGuard<'_, plan::Table> {
        self.table
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Every enabled provider extension's name, in registration order
    /// (FR-PROV-11's list for the generic `/login`).
    pub fn provider_names(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| entry.is_enabled() && entry.handle.worlds().contains(&World::Provider))
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
            .filter(|entry| entry.is_enabled())
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
                entry.is_enabled()
                    && entry.handle.name() == name
                    && entry.handle.worlds().contains(&World::Provider)
            })
            .map(|entry| &entry.handle)
    }

    /// Enable or disable one registered extension for the session
    /// (FR-PROV-9's disable knob; enablement itself lives in the grant
    /// store, per `docs/configuration.md`).
    pub fn set_enabled(&self, name: &str, enabled: bool) {
        for entry in &self.entries {
            if entry.handle.name() == name {
                entry
                    .enabled
                    .store(enabled, std::sync::atomic::Ordering::SeqCst);
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
                    Ok(Ok(usage)) => CommandEffect::ShowWidget(crate::quota::format_usage(&usage)),
                    Ok(Err(IdentityOutcome::NotSupported)) => {
                        // Gh #243: a bare refusal leaves users asking
                        // whether usage exists at all - name the missing
                        // quota endpoint and point at live token spending.
                        CommandEffect::ShowWidget(format!(
                            "usage is not supported by `{name}` (no account quota endpoint); session token spending is available via /session"
                        ))
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

    /// Every collision reported during registration or a later re-plan
    /// (FR-EXT-11's report).
    ///
    /// Owned rather than borrowed (gh #584): the log lives behind the table lock
    /// now, and a `&[..]` cannot outlive its guard. One `&` at the one caller.
    pub fn collisions(&self) -> Vec<CollisionReport> {
        self.lock_table().collisions.clone()
    }

    /// Enabled handles in registration order.
    pub fn enabled(&self) -> impl Iterator<Item = &Arc<dyn ExtensionDispatch>> {
        self.entries
            .iter()
            .filter(|entry| entry.is_enabled())
            .map(|entry| &entry.handle)
    }

    /// Whether this handle is enabled (FR-EXT-7's list data pairs it
    /// with `delivery()`).
    pub fn is_enabled(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.handle.name() == name && entry.is_enabled())
    }

    /// The extension that registered a bare tool name, if any.
    ///
    /// Re-plans first (gh #584), so a bridged tool whose server landed after
    /// registration resolves here - which is the half of the defect that
    /// answered `unknown tool`.
    pub fn tool_owner(&self, tool: &str) -> Option<&Arc<dyn ExtensionDispatch>> {
        self.refresh();
        let table = self.lock_table();
        let index = *table.tools.get(tool)?;
        // Borrowed out of the guard deliberately: the handle outlives the table,
        // and the entry index it names is the same one `refresh` just wrote.
        drop(table);
        self.entries.get(index).map(|entry| &entry.handle)
    }

    /// Whether a tool name is registered right now (gh #584's active set reads
    /// this instead of the table). Re-plans first, like every spec reader.
    pub(crate) fn has_tool(&self, name: &str) -> bool {
        self.refresh();
        self.lock_table().tool_schemas.contains_key(name)
    }

    /// The schema of a registered tool, for validating its arguments before
    /// dispatch (extension authoring guide: the host validates, then calls).
    pub fn tool_schema(&self, name: &str) -> Option<ToolSpec> {
        self.refresh();
        self.lock_table().tool_schemas.get(name).cloned()
    }

    /// The annotations a registered tool declares (#78): structural
    /// hints first (normalized from extras at registration), else
    /// `None`. The permission prompt may show them; nothing decides
    /// on them.
    pub fn tool_annotations(&self, name: &str) -> Option<ToolAnnotations> {
        self.refresh();
        self.lock_table()
            .tool_schemas
            .get(name)
            .and_then(|spec| spec.annotations.clone())
    }

    /// Whether the host may run a tool by name right now (gh #77's
    /// callable set): direct tools while active, codemode and deferred
    /// tools whenever registered; model-only and hidden tools never.
    pub fn is_callable(&self, name: &str) -> bool {
        self.refresh();
        let enabled = {
            let table = self.lock_table();
            table
                .tools
                .get(name)
                .map(|index| {
                    self.entries[*index]
                        .enabled
                        .load(std::sync::atomic::Ordering::SeqCst)
                })
                .unwrap_or(false)
        };
        if !enabled {
            return false;
        }
        let exposure = {
            let table = self.lock_table();
            table.tool_schemas.get(name).map(|spec| spec.exposure)
        };
        match exposure {
            Some(exposure) => match exposure {
                ToolExposure::Direct => self.is_active(name, exposure),
                ToolExposure::Codemode | ToolExposure::Deferred => true,
                ToolExposure::ModelOnly | ToolExposure::Hidden => false,
            },
            None => false,
        }
    }

    /// The callable tools for orchestrators (the `tools` import's
    /// `list-tools`, gh #77): name plus description, sorted.
    pub fn callable_names(&self) -> Vec<(String, String)> {
        self.refresh();
        // The names are collected under the lock and the predicate is asked
        // afterwards: `is_callable` takes the same lock, and `std::sync::Mutex`
        // is not re-entrant (gh #584).
        let specs: Vec<(String, String)> = self
            .lock_table()
            .tool_schemas
            .values()
            .map(|spec| (spec.name.clone(), spec.description.clone()))
            .collect();
        let mut names: Vec<(String, String)> = specs
            .into_iter()
            .filter(|(name, _)| self.is_callable(name))
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
    /// active, `direct` tools, plus `deferred` tools the model already
    /// discovered through `tool_search` (#55). Everything else reaches
    /// the model through orchestration, or not at all.
    pub fn declared_tool_specs(&self) -> Vec<ToolSpec> {
        let discovered = self
            .active
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .discovered
            .clone();
        let mut specs: Vec<ToolSpec> = self
            .tool_specs()
            .into_iter()
            .filter(|spec| {
                (spec.exposure == ToolExposure::Direct && self.is_active(&spec.name, spec.exposure))
                    || (spec.exposure == ToolExposure::Deferred && discovered.contains(&spec.name))
            })
            .collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
    }

    /// Every extension tool spec, for the provider's tool list. Sorted by
    /// name: a provider's prompt cache and the request-assembly snapshot key
    /// on stable bytes, and the registry's maps are unordered.
    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.refresh();
        let table = self.lock_table();
        let mut specs: Vec<ToolSpec> = table
            .tool_schemas
            .iter()
            .filter(|(name, _)| {
                table
                    .tools
                    .get(*name)
                    .map(|index| {
                        self.entries[*index]
                            .enabled
                            .load(std::sync::atomic::Ordering::SeqCst)
                    })
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
        self.refresh();
        let mut names: Vec<String> = self.lock_table().commands.keys().cloned().collect();
        names.sort();
        names
    }

    /// Invoke a full command name (the host namespaced it, it is a
    /// claimed built-in slot, or it is an auto-namespaced identity
    /// export, FR-PROV-10).
    pub fn invoke_command(&self, full: &str, argument: &str) -> Option<CommandEffect> {
        // Re-plans first (gh #584): a command a late-landing handle declares
        // becomes invocable too, and a withdrawn one stops being.
        self.refresh();
        let route = self.lock_table().commands.get(full).cloned()?;
        let entry = route.entry();
        let handle = self.entries.get(entry)?.handle.clone();
        if !self.entries[entry]
            .enabled
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return None;
        }
        match route {
            Route::World { leaf, .. } => handle.invoke_command(&leaf, argument).ok(),
            Route::Identity { op, .. } => Some(self.identity_effect(&handle, op)),
        }
    }

    /// Apply every enabled `context-transform` extension in
    /// installation order (FR-CTX-2), each output feeding the next. The
    /// first rejection - or host-level failure, which must not let
    /// unreviewed messages through either - stops the chain
    /// (FR-CTX-3).
    ///
    /// Gh #46 (pi's `restoreSystemMessages`): the `context` handlers
    /// own the conversation, not the prompt. After every handler the
    /// host re-attaches the leading system message when the handler
    /// dropped it, so a filter/slice cannot lose the prompt or the
    /// tool declarations and every later handler still sees the full
    /// transcript (pi's `context_with_system` visibility). An edited
    /// system head is honored - only a missing one is restored.
    pub async fn transform(
        &self,
        mut messages: Vec<ChatMessage>,
    ) -> Result<Vec<ChatMessage>, String> {
        // The prompt head the handlers must not lose: the leading
        // system run. Snapshotted once (pi replays the current head
        // per handler; the head cannot change mid-chain except through
        // a handler edit, which the per-output check below honors).
        let head: Vec<ChatMessage> = messages
            .iter()
            .take_while(|message| message.role == lca_protocol::MessageRole::System)
            .cloned()
            .collect();
        for handle in self
            .enabled()
            .filter(|handle| handle.worlds().contains(&World::ContextTransform))
        {
            // The clone keeps the input alive for a Disabled skip;
            // every other arm replaces or returns it anyway.
            match handle.transform_messages(messages.clone()).await {
                Ok(Ok(list)) => {
                    messages = list;
                    if !head.is_empty()
                        && !messages
                            .first()
                            .is_some_and(|first| first.role == lca_protocol::MessageRole::System)
                    {
                        tracing::warn!(
                            extension = handle.name(),
                            "context transform dropped the leading system message; the prompt was restored"
                        );
                        messages.splice(..0, head.iter().cloned());
                    }
                }
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
