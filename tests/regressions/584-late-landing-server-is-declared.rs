//! NFR-24 guard for gh #584: a server that lands after startup reaches the model.
//!
//! ## The defect
//!
//! `registry::assemble` registers the MCP bridge once, before any server has
//! connected (gh #418 Phase E made that the ordinary case on purpose: cold start
//! must stay inside NFR-3's 150 ms). `ExtensionRegistry::register` called
//! `handle.tool_specs()` exactly once and stored the result in `tool_schemas` and
//! `tools`, so a server that landed afterwards was **invisible in two places**:
//!
//! - `declared_tool_specs()` omitted it, so the model was never told the tool
//!   existed. Side by side against the same stdio server, `pi 1.3.0` declared it
//!   on the first request (`toolsAdded: … mcp__probe__probe`); LCA did not, for
//!   the whole session.
//! - `tool_owner(name)` answered `None`, so a call answered
//!   ``unknown tool `mcp__probe__probe` `` - while `/mcp` honestly said
//!   `connected (1 tools)` and `lca mcp list` (which connects synchronously)
//!   listed the tool.
//!
//! The first-turn `wait_for_startup` did not close the gap: it waits for the
//! *connection*, but the snapshot that fed the request had been taken before it.
//!
//! ## What this pins
//!
//! The behavioural rows live where the behaviour can be driven:
//!
//! - `crates/lca-core/tests/registry.rs`: a tool **and a command** that land after
//!   registration are declared, owned, invocable; two settled reads are
//!   byte-identical; a late withdrawal un-declares; a late name that is already
//!   owned loses and is reported (both against an earlier extension and against a
//!   built-in); a handle answering the trait's `None` default is never re-asked.
//! - `crates/lca-core/tests/mcp_bridge.rs`: the real bridge and the real registry
//!   - a slow server lands after `register`, is declared **and** callable **and**
//!     runs through the registry's dispatch; a late `hidden` server stays
//!     unreachable while a late `codemode` one is discoverable.
//! - `crates/lca-mcp/tests/startup.rs`: the version signal itself - it moves on a
//!   `land` and on a reconnect, and does **not** move on a read (a bump on read
//!   would make the registry re-plan on every `/mcp` paint and bust the prompt
//!   cache).
//!
//! What is left for this file is the one claim none of those can state: **every
//! mutation of the bridge's row set goes through the version-bumping accessor.**
//! A missed *read* is harmless, so a future edit could add a mutating site using
//! `locked()` and every behavioural row above would still pass on a fast machine
//! (the defect needs the landing to happen after the registry's snapshot, which
//! the rows do arrange - but only for the sites that exist today). The discipline
//! is pinned structurally, and the sites are countable so the next reader can
//! audit the set rather than trust it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use std::path::Path;

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|err| panic!("read {rel}: {err}"))
}

const NATIVE: &str = "crates/lca-mcp/src/native.rs";
/// Gh #548 split `crates/lca-core/src/registry.rs` into the module directory
/// `crates/lca-core/src/registry/` (`mod.rs` + `tools` + `hooks` + `plan` +
/// `manage`). Registration and the re-plan stayed in `mod.rs`, but a row that
/// names one **filename** stops seeing the code the day it moves - so this reads
/// the whole module through the shared walk (gh #574/#543) and only the planner
/// sibling is excluded, because the assertion below is precisely that the
/// reservation rules live there and nowhere else.
const REGISTRY_MODULE: &str = "crates/lca-core/src/registry/mod.rs";
const PLANNER: &str = "crates/lca-core/src/registry/plan.rs";

/// Every file of the registry module **except** the planner.
fn registry_without_plan() -> String {
    crate::module_sources(REGISTRY_MODULE)
        .into_iter()
        .filter(|(path, _)| path != PLANNER)
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of one method, sliced from its `fn` line to the closing `    }` that
/// ends it at method indentation.
fn method_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` exists in {NATIVE}"));
    let body = &source[start..];
    let end = body
        .find("\n    }\n")
        .unwrap_or_else(|| panic!("`{signature}` is a method body ending with an indented `}}`"));
    &body[..end]
}

// Verifies: gh #584 - `lock`, `batch`, `select`, `sql` and the resource trio all
// go through the version bump, and no mutating method still takes the plain read
// lock.
//
// The list is explicit on purpose: a new mutating site has to be added here,
// which is the moment its author is asked "did you mean `locked_mut`?".
#[test]
fn every_mutation_of_the_row_set_bumps_the_specs_version() {
    let source = read(NATIVE);
    for (label, signature) in [
        (
            "land",
            "pub(crate) fn land(&self, row: ManagedServer) -> bool {",
        ),
        (
            "rebuild",
            "pub fn rebuild(\n        &self,\n        caps: Arc<lca_tools::Capabilities>,",
        ),
        (
            "rebuild_connecting",
            "pub(crate) fn rebuild_connecting(\n        &self,",
        ),
        (
            "take_servers",
            "fn take_servers(self) -> Vec<ManagedServer> {",
        ),
        ("drop", "fn drop(&mut self) {"),
    ] {
        let body = method_body(&source, signature);
        assert!(
            body.contains("self.locked_mut()"),
            "`{label}` mutates the row set, so it must take the version-bumping \
             accessor (gh #584) - a landing that did not bump would leave a \
             connected server invisible to the model: {body}"
        );
        assert!(
            !body.contains("self.locked()"),
            "`{label}` must not also take the plain read lock for a mutation: {body}"
        );
    }
}

// Verifies: gh #584 - the version is bumped inside the one accessor, not at each
// call site, and reads are not bumped.
//
// "A missed read is harmless; a missed write is the only thing that breaks the
// signal": the check is that `locked_mut` is the *only* place `fetch_add` runs, so
// no site can bump by accident (which would re-plan on a paint) or forget to.
#[test]
fn the_version_is_bumped_in_one_place_and_never_on_a_read() {
    let source = read(NATIVE);
    // Two `self.servers_version` sites: the bump and the read accessor. The
    // field declaration and its two constructor initialisers are the third
    // through fifth mentions, and nothing else in the file touches it.
    assert_eq!(
        source.matches("self.servers_version").count(),
        2,
        "exactly one bump and one read, both in the accessor pair"
    );
    assert_eq!(
        source.matches("servers_version").count(),
        5,
        "the field, two constructor initialisers, and the accessor pair - no \
         other site touches it"
    );
    let mut_body = method_body(
        &source,
        "pub(crate) fn locked_mut(&self) -> std::sync::MutexGuard<'_, Vec<ManagedServer>> {",
    );
    assert!(
        mut_body.contains("self.servers_version") && mut_body.contains("fetch_add(1"),
        "the only bump lives in the mutating accessor: {mut_body}"
    );
    assert!(
        mut_body.contains("self.locked()"),
        "and it borrows the read lock rather than duplicating it: {mut_body}"
    );
    let read_body = method_body(
        &source,
        "pub(crate) fn locked(&self) -> std::sync::MutexGuard<'_, Vec<ManagedServer>> {",
    );
    assert!(
        !read_body.contains("fetch_add") && !read_body.contains("servers_version"),
        "a read must not bump, or every `/mcp` paint would re-plan the handle: {read_body}"
    );
}

// Verifies: gh #584 - the trait's version hook defaults to `None`, and the bridge
// overrides it.
//
// The default is the whole cost story: an additive method with a default keeps
// every fixed handle - every WASM guest, every in-tree extension - on the path it
// already had, with no re-plan and one atomic load. And it is an additive Rust
// change, not a `wit/` one: the host asks its own handles, so no guest sees it.
#[test]
fn the_trait_default_is_none_and_only_the_bridge_overrides_it() {
    // Gh #551 split the dispatch contract into `dispatch.rs`; read the module.
    let abi = crate::module_text("crates/lca-ext-abi/src/lib.rs");
    // `module_text` joins several files, so the closure is "the first `}` at the
    // declaration's own indent" rather than a literal `\n        }\n` (which a
    // rustfmt'd single-line `fn … { None }` would never match).
    let default_body = abi
        .split("fn specs_version(&self) -> Option<u64> {")
        .nth(1)
        .and_then(|rest| rest.split("\n    }\n").next())
        .expect("the trait's defaulted `specs_version` exists");
    assert!(
        default_body.trim() == "None",
        "the trait default answers `None` (my specs never change): {default_body:?}"
    );
    let native = read(NATIVE);
    assert!(
        native.contains(
            "fn specs_version(&self) -> Option<u64> {\n        Some(McpBridge::specs_version(self))"
        ),
        "the bridge answers with its own version"
    );
    // And no `wit/` file mentions it: this is host-side observation, not an
    // export a guest must implement.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wit = root.join("wit");
    let mut offending: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&wit) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("wit")
                && let Ok(text) = std::fs::read_to_string(&path)
                && text.contains("specs-version")
            {
                offending.push(path.display().to_string());
            }
        }
    }
    assert!(
        offending.is_empty(),
        "`specs_version` must not reach the WIT surface - it is the host asking \
         its own handles, not an export: {offending:?}"
    );
}

// Verifies: gh #584 - the re-plan reuses the registration rules instead of
// re-deriving them.
//
// The whole safety argument for a late re-plan is that it runs the *same* checks:
// built-in names and `tool_search` reserved, an earlier registration keeping a
// name, the native-only built-in slot claim. Two planners drifting apart is how a
// late handle would gain a privilege a registration could not - so `registry.rs`
// must not grow a second copy of the rules.
#[test]
fn the_replan_reuses_the_registration_planner() {
    let registry = registry_without_plan();
    let plan = read(PLANNER);
    assert!(
        plan.contains("BUILTIN_TOOLS.contains(&spec.name.as_str())"),
        "the reserved built-in check lives in the one planner"
    );
    assert!(
        plan.contains("|| spec.name == TOOL_SEARCH_NAME"),
        "and so does the `tool_search` reservation"
    );
    assert_eq!(
        registry.matches("plan::plan(").count(),
        2,
        "exactly two callers - registration and the re-plan - and no third \
         planner: {registry}"
    );
    assert!(
        !registry.contains("BUILTIN_TOOLS.contains(&spec.name.as_str())"),
        "the registry module outside the planner must not keep a second copy of \
         the reservation rules"
    );
    // The withdrawal is what lets a shrinking set shrink; without it a server
    // that went away would stay declared forever, and a reconnect would collide
    // with its own stale names. Both halves are checked: the method exists, and
    // the re-plan actually calls it before planning.
    assert!(
        plan.contains("pub(crate) fn withdraw(&mut self, entry: usize)"),
        "the table can withdraw one entry's names"
    );
    assert!(
        registry.contains("table.withdraw(index);"),
        "and the re-plan calls it, so a shrinking handle really shrinks"
    );
    // Both are searched inside the re-plan only: `register` has its own
    // `plan::plan(` call earlier in the file, so a file-wide search would be
    // satisfied by that one instead of by the re-plan's.
    let refresh = &registry[registry
        .find("pub(crate) fn refresh(&self)")
        .expect("the re-plan exists")..];
    let withdraw_call = refresh
        .find("table.withdraw(index);")
        .expect("checked just above");
    let plan_call = refresh.find("plan::plan(").expect("the re-plan plans");
    assert!(
        withdraw_call < plan_call,
        "the withdrawal must come **before** planning, or the handle collides \
         with its own previous names: withdraw={withdraw_call} plan={plan_call}"
    );
}
