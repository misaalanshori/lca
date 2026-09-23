//! The dispatch table (SRDD crate decomposition): loaded extension
//! handles in registration order, name namespaces, and collision
//! enforcement (FR-EXT-11). Handles are `Arc<dyn ExtensionDispatch>`, so
//! nothing here branches on the delivery mode (FR-EXT-6).

use std::collections::HashMap;
use std::sync::Arc;

use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_protocol::{
    CommandEffect, DispatchError, HookAction, IdentityOutcome, PostToolObservation, ToolCall,
    ToolSpec, Usage,
};

/// Built-in tool names are reserved (FR-TOOL-1's set; an extension
/// registering one is the later registration and loses, FR-EXT-11).
pub const BUILTIN_TOOLS: &[&str] = &["read", "write", "edit", "list", "glob", "grep", "shell"];

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
    /// Bare tool name -> entry index.
    tools: HashMap<String, usize>,
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

        let mut pending_tools: Vec<String> = Vec::new();
        let mut pending_commands: Vec<(String, PendingRoute)> = Vec::new();
        let native_claim = handle.delivery() == DeliveryMode::Native;
        let slots = handle.builtin_command_slots();

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
                                extension: name.clone(),
                                name: full,
                                kind: "command",
                                winner,
                            }),
                            None => pending_commands.push((full, PendingRoute::World(spec.name))),
                        }
                    }
                }
                Err(err) => {
                    // A handle that cannot enumerate its commands keeps its
                    // other worlds; report through the collision log.
                    self.collisions.push(CollisionReport {
                        extension: name.clone(),
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
                        extension: name.clone(),
                        name: full,
                        kind: "command",
                        winner,
                    }),
                    None => pending_commands.push((full, PendingRoute::Identity(op))),
                }
            }
        }

        if handle.worlds().contains(&World::Tool) {
            match handle.tool_specs() {
                Ok(specs) => {
                    for spec in specs {
                        let winner = if BUILTIN_TOOLS.contains(&spec.name.as_str()) {
                            Some("built-in".to_string())
                        } else {
                            self.tools
                                .get(&spec.name)
                                .map(|index| self.entry_name(*index).to_string())
                        };
                        match winner {
                            Some(winner) => self.collisions.push(CollisionReport {
                                extension: name.clone(),
                                name: spec.name,
                                kind: "tool",
                                winner,
                            }),
                            None => {
                                // The entry index is not pushed yet: patch
                                // after push via the pending name below.
                                pending_tools.push(spec.name);
                            }
                        }
                    }
                }
                Err(err) => {
                    self.collisions.push(CollisionReport {
                        extension: name.clone(),
                        name: "(tool world)".to_string(),
                        kind: "tool",
                        winner: format!("unavailable: {err}"),
                    });
                }
            }
        }

        let entry_index = self.entries.len();
        for tool in pending_tools {
            self.tools.insert(tool, entry_index);
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
    /// thread: command invocation runs outside any async context (the
    /// TUI is synchronous there), so a fresh current-thread runtime is
    /// safe and needs no runtime handle plumbing.
    /// ponytail: panics if a caller ever invokes this from inside an
    /// async task; route such a caller through `spawn` + a channel.
    fn drive<T>(&self, future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("identity runtime")
            .block_on(future)
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

    /// Every extension tool spec, for the provider's tool list.
    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.tools
            .values()
            .flat_map(|index| {
                let entry = &self.entries[*index];
                if entry.enabled {
                    entry.handle.tool_specs().unwrap_or_default()
                } else {
                    Vec::new()
                }
            })
            .collect()
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

    /// Force any running WASM call to trap (epoch interruption,
    /// FR-CONC-1); native handles share the caller's cancellation flag.
    pub fn interrupt_all(&self) {
        for handle in self.enabled() {
            handle.interrupt();
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
