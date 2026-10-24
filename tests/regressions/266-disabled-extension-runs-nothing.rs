//! GitHub #266: a disabled extension must run **nothing** - and must stay
//! disabled however late it is registered.
//!
//! # The defect
//!
//! `registry::assemble` registered the MCP bridge *after* `apply_enablement` had
//! swept the registry, so `mcp` was never consulted against the project's
//! disablement. Two consequences, and the second is the serious one: the setting
//! had no effect, **and `McpManager::load` spawned every configured stdio child and
//! opened every remote connection anyway**. A user said no and the program did it
//! regardless.
//!
//! The primary half landed earlier as `16fc841e` (#418 Phase E): enablement is read
//! before anything connects, and a disabled MCP's entries are marked `enabled =
//! false` so `spawn_connections` creates a `Disabled` row instead of a worker
//! thread. `crates/lca-cli/tests/mcp_enablement.rs` guards that with a spawn marker
//! file - a direct observation of "no child existed", not an inference from a tool
//! list that could have been filtered afterwards.
//!
//! # The half this file guards
//!
//! The same shape survived in a second place, which is why the brief demanded a
//! sibling sweep. `registry::register_compaction` registers into a **live** registry
//! in `Ui::new` and again on `/reload`. `assemble` has already swept by then, and
//! `tui/hooks.rs` - the `/reload` path - contains **no `apply_enablement` call at
//! all**. So a disabled `compaction-default` was silently re-enabled on every
//! reload: the decision did not precede the registration, exactly as in #266.
//!
//! Ordering the calls cannot fix that, because there is no later sweep to order it
//! before. The seam is `registry::register_gated`: register, then apply the
//! predicate to that one name. A future registration path cannot reintroduce the
//! bug by forgetting a sweep, because the sweep travels with the registration.
//!
//! Behavioural rows for the gate live beside it in
//! `crates/lca-cli/tests/mcp_enablement.rs`; this file is the NFR-24 named guard,
//! asserting the properties that a behavioural row on one call path cannot reach.

use std::path::Path;

fn read(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// The body of `register_compaction`'s bundled arm, sans the no-bundled twin.
fn register_compaction_body(source: &str) -> &str {
    source
        .split("pub(crate) fn register_compaction(")
        .nth(1)
        .and_then(|rest| rest.split("/// No bundled compaction").next())
        .expect("register_compaction exists")
}

// Verifies: gh #266 - **no registration site in `lca-cli` registers a handle
// without gating it.** The sibling sweep, as a row: the issue named one instance
// and the fix has to cover the seam, so every `registry.register(` in the crate is
// enumerated here. A new call site is a red row, which is the only way a guard can
// stay ahead of the next ordering mistake.
#[test]
fn every_registration_site_consults_enablement() {
    // Each exclusion is a *decision with a reason*, not a grep: a site belongs here
    // only if a sweep provably follows its registration in that same function.
    // Adding a file to this list without checking that is how the bug comes back.
    const SWEPT_LATER: &[&str] = &[
        // `assemble` registers the loaders' handles, the native set, the bundled
        // provider and the MCP bridge, then sweeps once at the end.
        "crates/lca-cli/src/registry.rs",
        "crates/lca-cli/src/project_ext.rs", // load_project_local, swept by assemble
        "crates/lca-cli/src/ext.rs",         // load_installed / load_extra, ditto
        // Headless registers the compaction handle at line 946 and calls
        // `apply_enablement` at 951 - five lines later, in the same function, after
        // every registration. Correct order today; if that sweep ever moves, this
        // row stops being the thing that notices, so the comment is the guard.
        "crates/lca-cli/src/headless.rs",
    ];
    // **Recursive**, and that is load-bearing: my first version read only the top
    // level of `src/`, so it never visited `src/tui/hooks.rs` - the one file whose
    // late registration the whole fix is about. The row was green while the defect
    // it exists to catch sat one directory down. A guard that scans half the crate
    // is worse than no guard, because it reads as coverage.
    fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-cli/src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(
        files.len() > 20,
        "the walk found the crate: {}",
        files.len()
    );
    let mut unguarded: Vec<String> = Vec::new();
    for path in files {
        let name = path
            .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("prefix")
            .to_string_lossy()
            .replace('\\', "/");
        if SWEPT_LATER.contains(&name.as_str()) {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read source");
        // Test modules are cut: a `#[cfg(test)]` stub registry is a fixture, not a
        // production registration path, and `model_select.rs`'s own rows build one.
        // Cut at the first `#[cfg(test)]` on a `mod`, which is where every such
        // block in this crate starts.
        let production = source.split("#[cfg(test)]\nmod ").next().unwrap_or(&source);
        for (index, line) in production.lines().enumerate() {
            if line.contains("registry.register(") || line.contains("inner.register(") {
                unguarded.push(format!("{name}:{}: {}", index + 1, line.trim()));
            }
        }
    }
    assert!(
        unguarded.is_empty(),
        "a registration outside assemble's sweep must go through `register_gated` \
         (gh #266: the decision has to precede the side effect):\n{}",
        unguarded.join("\n")
    );
}

// Verifies: gh #266 - **the gate is applied inside `register_gated`, in the right
// order.** Register-then-gate is the only order that works: a predicate consulted
// *before* `register` would have to know the name, and one applied only afterwards
// by someone else is exactly the missing-sweep bug. Asserted on position, because a
// behavioural row cannot distinguish "gated at registration" from "gated by luck".
#[test]
fn the_gate_runs_at_the_registration() {
    let registry = read("crates/lca-cli/src/registry.rs");
    let gated = registry
        .split("pub fn register_gated(")
        .nth(1)
        .and_then(|rest| rest.split("/// A bare `CompactionDefault` handle").next())
        .unwrap_or_else(|| panic!("register_gated exists"));
    let register_at = gated
        .find("registry.register(handle)")
        .expect("it registers");
    let disable_at = gated
        .find("set_enabled(&name, false)")
        .expect("and it disables immediately");
    assert!(
        disable_at > register_at,
        "the disablement follows the registration, so the handle exists to disable:\n{gated}"
    );
    let predicate_at = gated
        .find("disabled_here(&name)")
        .expect("it consults the predicate");
    assert!(
        predicate_at < register_at,
        "the decision is read before the handle is installed, so nothing runs in \
         between:\n{gated}"
    );
}

// Verifies: gh #266 - **`register_compaction` cannot bypass the gate.** It is the
// one registration that happens after `assemble`'s sweep (`Ui::new`) and on
// `/reload`, so it is the site the defect lived in. Asserted both ways: it calls
// the seam, and it has no bare `registry.register`.
#[test]
fn the_late_registration_goes_through_the_gate() {
    let registry = read("crates/lca-cli/src/registry.rs");
    let compaction = register_compaction_body(&registry);
    assert!(
        compaction.contains("register_gated("),
        "the compaction registration applies enablement itself:\n{compaction}"
    );
    assert!(
        !compaction.contains("registry.register("),
        "and never registers bare:\n{compaction}"
    );
}

// Verifies: gh #266 - **`/reload` has no sweep of its own, which is why the gate
// must travel with the registration.** This is the observation that made the
// sibling real rather than theoretical: the file that registers a handle after
// `assemble` never calls `apply_enablement`. Pinned so that if a sweep is ever
// added there, this row's comment is the thing that explains why it is not needed.
#[test]
fn the_reload_path_registers_without_a_sweep_of_its_own() {
    let hooks = read("crates/lca-cli/src/tui/hooks.rs");
    assert!(
        hooks.contains("register_compaction("),
        "`/reload` registers the compaction handle into a live registry"
    );
    assert!(
        !hooks.contains("apply_enablement("),
        "and has no sweep of its own - so `register_gated` is what keeps the \
         registered handle honest (gh #266)"
    );
}

// Verifies: gh #266 - **the primary defect stays fixed.** The enablement decision
// is read before `McpManager::load` exists, so a disabled MCP cannot spawn: the
// entries are marked disabled *before* the manager is constructed, and
// `spawn_connections` turns a disabled entry into a `Disabled` row rather than a
// worker thread. Asserted on the order inside `assemble` and on the guard in the
// spawner, which together are the property "nothing runs".
#[test]
fn a_disabled_mcp_cannot_reach_the_spawner() {
    let registry = read("crates/lca-cli/src/registry.rs");
    let assemble = registry
        .split("pub(crate) fn assemble(")
        .nth(1)
        .and_then(|rest| rest.split("/// A bare `CompactionDefault` handle").next())
        .expect("assemble exists");
    let decision_at = assemble
        .find("extension_enabled(cwd, \"mcp\")")
        .expect("the MCP enablement decision is read in assemble");
    let disable_entries_at = assemble
        .find("entry.enabled = false")
        .expect("and a disabled decision marks the entries");
    let load_at = assemble
        .find("McpManager::load(")
        .expect("the manager is constructed");
    assert!(
        decision_at < disable_entries_at && disable_entries_at < load_at,
        "read the decision, apply it to the entries, then construct the manager - \
         in that order, or a disabled MCP reaches the spawner:\n{assemble}"
    );

    // And the spawner honours the flag, which is the half that makes the order
    // matter: a `Disabled` row is pushed instead of a worker thread.
    let native = read("crates/lca-mcp/src/native.rs");
    let spawn = native
        .split("pub(crate) fn spawn_connections(")
        .nth(1)
        .and_then(|rest| rest.split("/// Land one finished row").next())
        .expect("spawn_connections exists");
    let guard_at = spawn
        .find("if !entry.enabled")
        .expect("the spawner checks the flag");
    let worker_at = spawn
        .find("McpBridge::connect_entry(")
        .expect("and connects the others");
    assert!(
        guard_at < worker_at,
        "a disabled entry is turned into a row before any worker is spawned:\n{spawn}"
    );
}
