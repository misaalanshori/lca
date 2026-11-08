//! Name planning (gh #584): the registry's tables and the collision rules,
//! in one place so a handle re-planned **after** registration runs exactly
//! the checks `register` runs. Split from `registry.rs` for the file ceiling;
//! the tables stay owned by `ExtensionRegistry` (one `Mutex<Table>`, no
//! second copy of anything).
//!
//! The division of labour is deliberate:
//!
//! - [`fetch`] calls the handle. It is foreign, possibly blocking (a WASM
//!   handle runs one component call) and possibly re-entrant (a guest may ask
//!   the host's `ToolsRegistryView` from inside `tool_specs`), so it runs
//!   **outside** the table lock.
//! - [`plan`] is pure: it reads the table and the handle's answer and returns
//!   what should be installed. No lock, no mutation, so it cannot deadlock.
//! - [`Table::install`] and [`Table::withdraw`] are the mutations, under the
//!   caller's lock.

use std::collections::HashMap;

use lca_ext_abi::{ExtensionDispatch, World};
use lca_protocol::{CommandSpec, DispatchError, ToolSpec};

use super::{
    BUILTIN_COMMANDS, BUILTIN_TOOL_ALIASES, BUILTIN_TOOLS, CollisionReport, IdentityCommand,
    Registered, TOOL_SEARCH_NAME,
};

/// Where a full command name leads: the extension's `command` world, or
/// one of its `provider`-world identity exports (FR-PROV-10).
#[derive(Debug, Clone)]
pub(crate) enum Route {
    World { entry: usize, leaf: String },
    Identity { entry: usize, op: IdentityCommand },
}

impl Route {
    /// The entry that answers this route.
    pub(crate) fn entry(&self) -> usize {
        match self {
            Route::World { entry, .. } | Route::Identity { entry, .. } => *entry,
        }
    }
}

/// A planned route before its entry index is known (see `plan`).
pub(crate) enum PendingRoute {
    World(String),
    Identity(IdentityCommand),
}

/// The registry's name tables (gh #584): behind one lock so a re-plan,
/// a lookup and a declaration all see the same snapshot. There is exactly
/// one of each - a per-handle spec cache beside these would be a second
/// source of truth.
#[derive(Default)]
pub(crate) struct Table {
    /// Bare tool name -> entry index.
    pub(crate) tools: HashMap<String, usize>,
    /// The kept spec per registered tool name, so arguments can be validated
    /// against the schema the model saw before `execute` runs.
    pub(crate) tool_schemas: HashMap<String, ToolSpec>,
    /// Full command name (`ext.command`, a claimed built-in leaf, or an
    /// auto-namespaced identity export) -> where it leads.
    pub(crate) commands: HashMap<String, Route>,
    /// The collision log (FR-EXT-11's report).
    pub(crate) collisions: Vec<CollisionReport>,
    /// The specs version each entry was planned from (gh #584): the watermark
    /// that decides whether a re-plan is needed. No entry means "planned at
    /// registration, from the handle's `None` default" - a fixed handle is
    /// never re-planned and costs one atomic load.
    pub(crate) planned: Vec<u64>,
}

/// What one handle answered, fetched **outside** the table lock (gh #584).
pub(crate) struct Fetch {
    tools: Result<Vec<ToolSpec>, DispatchError>,
    commands: Result<Vec<CommandSpec>, DispatchError>,
}

/// What [`plan`] decided, ready to install.
pub(crate) struct Planned {
    tools: Vec<ToolSpec>,
    commands: Vec<(String, PendingRoute)>,
    /// The collisions this handle lost (FR-EXT-11's report).
    pub(crate) reports: Vec<CollisionReport>,
}

impl Table {
    /// Record a collision outside any planning pass (duplicate identities,
    /// which are decided before a handle is planned at all).
    pub(crate) fn insert_collision(&mut self, report: CollisionReport) {
        self.collisions.push(report);
    }

    /// Append the reports one plan produced.
    pub(crate) fn insert_collisions(&mut self, reports: Vec<CollisionReport>) {
        self.collisions.extend(reports);
    }

    /// Remember which version this entry's names were planned from. Written
    /// with the install, so a reader sees the names and the watermark together.
    pub(crate) fn note_planned(&mut self, entry: usize, version: u64) {
        if self.planned.len() <= entry {
            self.planned.resize(entry + 1, 0);
        }
        self.planned[entry] = version;
    }

    /// Install one handle's planned names.
    pub(crate) fn install(&mut self, entry: usize, planned: Planned) {
        for spec in planned.tools {
            self.tools.insert(spec.name.clone(), entry);
            self.tool_schemas.insert(spec.name.clone(), spec);
        }
        for (full, pending) in planned.commands {
            let route = match pending {
                PendingRoute::World(leaf) => Route::World { entry, leaf },
                PendingRoute::Identity(op) => Route::Identity { entry, op },
            };
            self.commands.insert(full, route);
        }
    }

    /// Remove every name this entry owns **before** it is re-planned (gh #584).
    ///
    /// Without this a shrinking tool set could never shrink (the old names
    /// would stay registered), and a handle would collide with its own
    /// previous names. With it, the asymmetry FR-EXT-11 already has is preserved
    /// exactly: only *this* entry's names are released, so a name another
    /// handle owns stays with it, and the re-plan then loses it the same way
    /// `register` would.
    pub(crate) fn withdraw(&mut self, entry: usize) {
        let owned: Vec<String> = self
            .tools
            .iter()
            .filter(|(_, index)| **index == entry)
            .map(|(name, _)| name.clone())
            .collect();
        for name in owned {
            self.tools.remove(&name);
            self.tool_schemas.remove(&name);
        }
        self.commands.retain(|_, route| route.entry() != entry);
    }
}

/// Ask one handle for its specs, outside any lock. A world the handle does
/// not declare is not asked at all, which is what keeps `register` from
/// reporting a `MissingWorld` error as a collision for every non-command
/// handle.
pub(crate) fn fetch(handle: &dyn ExtensionDispatch, worlds: &[World]) -> Fetch {
    let tools = if worlds.contains(&World::Tool) || worlds.contains(&World::ToolCatalog) {
        handle.tool_specs()
    } else {
        Ok(Vec::new())
    };
    let commands = if worlds.contains(&World::Command) {
        handle.command_specs()
    } else {
        Ok(Vec::new())
    };
    Fetch { tools, commands }
}

/// Plan one handle's names against the table, mutating nothing.
///
/// The rules are the registration rules, verbatim: built-in names,
/// pi-name aliases and `tool_search` are reserved (FR-TOOL-1, gh #119,
/// gh #77), an earlier registration keeps a name another handle wants
/// (FR-EXT-11), and only a native handle claiming one of the documented
/// built-in slots gets the bare command name (ADR-0019).
pub(crate) fn plan(
    table: &Table,
    entries: &[Registered],
    name: &str,
    worlds: &[World],
    native_claim: bool,
    slots: &[String],
    fetched: Fetch,
) -> Planned {
    let mut planned = Planned {
        tools: Vec::new(),
        commands: Vec::new(),
        reports: Vec::new(),
    };
    plan_tools(table, entries, name, worlds, fetched.tools, &mut planned);
    plan_commands(
        table,
        entries,
        name,
        worlds,
        native_claim,
        slots,
        fetched.commands,
        &mut planned,
    );
    planned
}

fn plan_tools(
    table: &Table,
    entries: &[Registered],
    name: &str,
    worlds: &[World],
    fetched: Result<Vec<ToolSpec>, DispatchError>,
    planned: &mut Planned,
) {
    if !(worlds.contains(&World::Tool) || worlds.contains(&World::ToolCatalog)) {
        return;
    }
    match fetched {
        Ok(specs) => {
            for spec in specs {
                let winner = if BUILTIN_TOOLS.contains(&spec.name.as_str())
                    || BUILTIN_TOOL_ALIASES.contains(&spec.name.as_str())
                    || spec.name == TOOL_SEARCH_NAME
                {
                    Some("built-in".to_string())
                } else {
                    table
                        .tools
                        .get(&spec.name)
                        .and_then(|index| entries.get(*index))
                        .map(|entry| entry.handle.name().to_string())
                };
                match winner {
                    Some(winner) => planned.reports.push(CollisionReport {
                        extension: name.to_string(),
                        name: spec.name,
                        kind: "tool",
                        winner,
                    }),
                    // #78: single-tool guests declare hints through extras;
                    // normalize on the way in so every consumer sees one shape.
                    None => planned.tools.push(spec.with_annotations_from_extras()),
                }
            }
        }
        Err(err) => {
            // A handle that cannot enumerate its tools keeps its other worlds;
            // report through the collision log.
            planned.reports.push(CollisionReport {
                extension: name.to_string(),
                name: "(tool world)".to_string(),
                kind: "tool",
                winner: format!("unavailable: {err}"),
            });
        }
    }
}

#[allow(clippy::too_many_arguments)] // every argument is one fact from the handle; grouping them into a struct here would be a fourth parameter shape to keep in sync.
fn plan_commands(
    table: &Table,
    entries: &[Registered],
    name: &str,
    worlds: &[World],
    native_claim: bool,
    slots: &[String],
    fetched: Result<Vec<CommandSpec>, DispatchError>,
    planned: &mut Planned,
) {
    if worlds.contains(&World::Command) {
        match fetched {
            Ok(specs) => {
                for spec in specs {
                    // Only a native first-party handle may claim a reserved
                    // built-in slot, and only for one of the documented built-in
                    // names (ADR-0019); everything else namespaces as
                    // `<extension>.<command>` (ADR-0012's mechanism), so a
                    // third-party command can never shadow a built-in.
                    let claims_builtin = native_claim
                        && slots.contains(&spec.name)
                        && BUILTIN_COMMANDS.contains(&spec.name.as_str());
                    let full = if claims_builtin {
                        spec.name.clone()
                    } else {
                        format!("{name}.{}", spec.name)
                    };
                    let winner = table
                        .commands
                        .get(&full)
                        .and_then(|route| entries.get(route.entry()))
                        .map(|entry| entry.handle.name().to_string());
                    match winner {
                        Some(winner) => planned.reports.push(CollisionReport {
                            extension: name.to_string(),
                            name: full,
                            kind: "command",
                            winner,
                        }),
                        None => planned
                            .commands
                            .push((full, PendingRoute::World(spec.name))),
                    }
                }
            }
            Err(err) => {
                planned.reports.push(CollisionReport {
                    extension: name.to_string(),
                    name: "(command world)".to_string(),
                    kind: "command",
                    winner: format!("unavailable: {err}"),
                });
            }
        }
    }

    if worlds.contains(&World::Provider) {
        // FR-PROV-10 / ADR-0012: the host namespaces the three identity
        // exports itself, so two providers that both export `usage` can never
        // collide.
        for (leaf, op) in [
            ("login", IdentityCommand::Login),
            ("logout", IdentityCommand::Logout),
            ("usage", IdentityCommand::Usage),
        ] {
            let full = format!("{name}.{leaf}");
            let winner = table
                .commands
                .get(&full)
                .and_then(|route| entries.get(route.entry()))
                .map(|entry| entry.handle.name().to_string());
            match winner {
                Some(winner) => planned.reports.push(CollisionReport {
                    extension: name.to_string(),
                    name: full,
                    kind: "command",
                    winner,
                }),
                None => planned.commands.push((full, PendingRoute::Identity(op))),
            }
        }
    }
}
