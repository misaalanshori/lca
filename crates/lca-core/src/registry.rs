//! The dispatch table (SRDD crate decomposition): loaded extension
//! handles in registration order, name namespaces, and collision
//! enforcement (FR-EXT-11). Handles are `Arc<dyn ExtensionDispatch>`, so
//! nothing here branches on the delivery mode (FR-EXT-6).

use std::collections::HashMap;
use std::sync::Arc;

use lca_ext_abi::{DeliveryMode, ExtensionDispatch, World};
use lca_protocol::{
    CommandEffect, DispatchError, HookAction, PostToolObservation, ToolCall, ToolSpec,
};

/// Built-in tool names are reserved (FR-TOOL-1's set; an extension
/// registering one is the later registration and loses, FR-EXT-11).
pub const BUILTIN_TOOLS: &[&str] = &["read", "write", "edit", "list", "glob", "grep", "shell"];

/// Built-in slash command names, reserved the same way (SRDD's list).
pub const BUILTIN_COMMANDS: &[&str] = &["login", "logout", "usage", "model", "compact", "stats"];

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
    /// Full command name (`ext.command`, or a claimed built-in leaf) ->
    /// (entry index, leaf).
    commands: HashMap<String, (usize, String)>,
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
        let mut pending_commands: Vec<(String, String)> = Vec::new();
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
                            .map(|(index, _)| self.entry_name(*index).to_string());
                        match winner {
                            Some(winner) => self.collisions.push(CollisionReport {
                                extension: name.clone(),
                                name: full,
                                kind: "command",
                                winner,
                            }),
                            None => pending_commands.push((full, spec.name)),
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
        for (full, leaf) in pending_commands {
            self.commands.insert(full, (entry_index, leaf));
        }
        self.entries.push(Registered {
            handle,
            enabled: true,
        });
    }

    fn entry_name(&self, index: usize) -> &str {
        self.entries[index].handle.name()
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

    /// Invoke a full command name (the host namespaced it, or it is a
    /// claimed built-in slot).
    pub fn invoke_command(&self, full: &str, argument: &str) -> Option<CommandEffect> {
        let (index, leaf) = self.commands.get(full)?;
        let entry = &self.entries[*index];
        if !entry.enabled {
            return None;
        }
        entry.handle.invoke_command(leaf, argument).ok()
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
