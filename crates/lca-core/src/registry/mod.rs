//! The dispatch table (SRDD crate decomposition): loaded extension
//! handles in registration order, name namespaces, and collision
//! enforcement (FR-EXT-11). Handles are `Arc<dyn ExtensionDispatch>`, so
//! nothing here branches on the delivery mode (FR-EXT-6).
//!
//! **This module is a partition, not a single file** (gh #548). Gate 11 caps a
//! file at 1,200 lines and this one was 41 lines from it, so the parts that
//! answer their own question moved beside it:
//!
//! - `mod.rs` (here): registration and collisions - `register`, the re-plan
//!   (`refresh`), the entry list, enablement, and the command routes;
//! - [`tools`]: tool exposure and the tools-import surface - which specs
//!   declare, `tool_search`'s discovery, the namespace grouping, and the
//!   `ToolsRegistryView` the `tools` import reaches through the host;
//! - [`hooks`]: the hook chains - pre/post turn, the tool-call and tool-result
//!   composition, settle, compaction, cache warming, trust, stream observation,
//!   session close, and the context-transform chain. **A reader looking for
//!   capability enforcement should follow the hook chain there**: every one of
//!   them runs over `enabled()` and a failure disables or skips rather than ends
//!   (FR-EXT-3);
//! - [`plan`]: the name tables and the collision rules, so a late re-plan runs
//!   exactly the checks `register` runs (gh #584);
//! - [`manage`]: the compaction preference and the manager's rows (gh #3).
//!
//! The state stays here: one `entries` list, one `table`, one `active` set. The
//! children are methods on this type, not a second registry, and every direction
//! they reach (`super::ExtensionRegistry`) is a sibling's use of the same reach.

use crate::active_set::ActiveSet;
use std::collections::HashMap;
use std::sync::Arc;

use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_protocol::{CommandEffect, IdentityOutcome};

/// The name tables plus the planning rules (gh #584), split for the file
/// ceiling. One `Table`, one set of rules, shared by registration and re-plan.
mod plan;
use plan::Route;

/// The hook chains (gh #548).
mod hooks;
/// Tool exposure and the tools-import surface (gh #548).
mod tools;
/// A namespace and the tools grouped under it (gh #77's grouping:
/// related tools list under one heading with the description).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
/// ceiling. Since gh #548 it is an ordinary child of this module
/// (`registry/manage.rs`): the `#[path]` attribute it used to carry was
/// how a plain `registry.rs` reached a sibling file, and a
/// `registry/mod.rs` declares children by `mod` like any other module.
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
