//! gh #418 Phase E (gh #266): a disabled MCP must never spawn a server, and
//! enabling/disabling must be consulted before anything connects.
//!
//! These rows drive the **real binary** (`CARGO_BIN_EXE_lca`) against the real
//! fixture, because the defect is an ordering bug inside `registry::assemble`:
//! a unit row on `McpManager` would pass while the host still spawned first.
//! The fixture appends to `$LCA_SPAWN_MARKER` as its very first statement, so
//! "zero spawns" is a direct observation of the filesystem — not an inference
//! from a tool list that could have been filtered afterwards.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

mod common;

use std::path::{Path, PathBuf};

use common::{Reply, rt, sandbox, sse_text, start_mock};

/// The spawn-recording fixture (gh #418 Phase E).
fn fixture() -> String {
    // Forward slashes everywhere: a Windows backslash path would read as a
    // JSON escape inside `mcp.json`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../lca-mcp/fixtures/spawn-probe-mcp-server.py")
        .to_string_lossy()
        .replace('\\', "/")
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// An `mcp.json` naming the spawn-probe fixture.
fn write_user_mcp(home: &Path, extra: &str) {
    let path = home.join(".lca").join("mcp.json");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &path,
        format!(
            r#"{{"mcpServers": {{"spawnprobe": {{"command": "python3", "args": ["{}"]{extra}}}}}}}"#,
            fixture()
        ),
    )
    .expect("write mcp.json");
}

/// The marker contents, or an empty string when the fixture never ran.
fn spawns(marker: &Path) -> String {
    std::fs::read_to_string(marker).unwrap_or_default()
}

// Verifies: gh #418 Phase E (gh #266) - the turn-level guard. A session whose
// grant store marks `mcp` disabled runs a turn, and the fixture proves **zero
// child spawns**: its marker file must not exist. Before the fix
// `McpManager::load` ran *after* `apply_enablement`, so the bridge spawned
// every configured server regardless of the disablement record.
#[test]
fn a_disabled_mcp_spawns_no_server_and_the_turn_still_runs() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let sandbox = sandbox("mcp-disabled-no-spawn");
    let marker = sandbox.root.join("spawns.txt");
    write_user_mcp(&sandbox.home, "");
    // The disablement record: `mcp` is off for this project.
    sandbox.approve_loopback_net(serde_json::json!({
        "extensions": { "mcp": false },
    }));

    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("done without mcp"))]));
    let output = sandbox.run_env(
        Some(&mock),
        &["-p", "--mode", "json", "say hi"],
        &[("LCA_SPAWN_MARKER", marker.to_str().expect("utf8"))],
    );
    assert_eq!(
        spawns(&marker),
        "",
        "a disabled MCP must spawn nothing, but the fixture recorded: {:?}",
        spawns(&marker)
    );
    assert!(
        output.status.success(),
        "the turn runs without MCP: {output:?}"
    );
    // And no MCP tool was offered to the model.
    let seen = common::stdout(&output);
    assert!(
        !seen.contains("mcp__spawnprobe__probe"),
        "no MCP tool reaches the model: {seen}"
    );
}

// Verifies: gh #418 Phase E - the same proof for the per-run flag. `--no-mcp`
// never writes anything (it is not a persisted setting), and it spawns
// nothing either.
#[test]
fn the_no_mcp_flag_spawns_no_server_and_persists_nothing() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let sandbox = sandbox("mcp-no-mcp-flag");
    let marker = sandbox.root.join("spawns.txt");
    write_user_mcp(&sandbox.home, "");
    sandbox.approve_loopback_net(serde_json::json!({}));
    let grants_before =
        std::fs::read_to_string(sandbox.state_dir().join("grants.json")).expect("grants");

    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("done without mcp"))]));
    let output = sandbox.run_env(
        Some(&mock),
        &["-p", "--mode", "json", "--no-mcp", "say hi"],
        &[("LCA_SPAWN_MARKER", marker.to_str().expect("utf8"))],
    );
    assert_eq!(
        spawns(&marker),
        "",
        "`--no-mcp` must spawn nothing, but the fixture recorded: {:?}",
        spawns(&marker)
    );
    assert!(output.status.success(), "the turn runs: {output:?}");
    // Per-run only: nothing was written to the grant store.
    let grants_after =
        std::fs::read_to_string(sandbox.state_dir().join("grants.json")).expect("grants");
    assert_eq!(
        grants_before, grants_after,
        "`--no-mcp` is a per-run flag, never a persisted setting"
    );
}

// The control row: with MCP enabled and no flag, the server DOES spawn and its
// tool reaches the model. Without this, the two rows above could pass because
// the fixture never runs at all.
#[test]
fn an_enabled_mcp_spawns_the_server_and_offers_its_tool() {
    if !python3_available() {
        eprintln!("skip: python3 is not installed");
        return;
    }
    let sandbox = sandbox("mcp-enabled-spawns");
    let marker = sandbox.root.join("spawns.txt");
    write_user_mcp(&sandbox.home, r#", "exposure": "direct""#);
    sandbox.approve_loopback_net(serde_json::json!({}));

    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("done"))]));
    let output = sandbox.run_env(
        Some(&mock),
        &["-p", "--mode", "json", "--yolo", "say hi"],
        &[("LCA_SPAWN_MARKER", marker.to_str().expect("utf8"))],
    );
    assert!(output.status.success(), "the turn runs: {output:?}");
    assert!(
        !spawns(&marker).is_empty(),
        "an enabled MCP spawns the server (the control the other rows need)"
    );
}

// Verifies: gh #418 Phase E - enablement is consulted before anything connects,
// so a disabled MCP also does not connect to a *remote* server. The marker
// cannot observe a remote host, so this row asserts the HTTP endpoint was never
// asked by making it an unroutable address: with the fix the run is quiet,
// without it the connect attempt would record a failure in `/mcp`'s state.
#[test]
fn a_disabled_mcp_does_not_connect_to_a_remote_server() {
    let sandbox = sandbox("mcp-disabled-remote");
    let path = sandbox.home.join(".lca").join("mcp.json");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &path,
        r#"{"mcpServers": {"remote": {"url": "http://127.0.0.1:9/mcp", "oauth": {}}}}"#,
    )
    .expect("write mcp.json");
    sandbox.approve_loopback_net(serde_json::json!({
        "extensions": { "mcp": false },
    }));

    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("done"))]));
    let output = sandbox.run(Some(&mock), &["-p", "--mode", "json", "say hi"]);
    assert!(output.status.success(), "the turn runs: {output:?}");
    let text = common::stderr(&output) + &common::stdout(&output);
    assert!(
        !text.contains("remote") || !text.contains("failed"),
        "a disabled MCP connects to nothing, so no connect failure is reported: {text}"
    );
}

// ===========================================================================
// gh #266, the sibling: a handle registered *after* the sweep
// ===========================================================================
//
// The rows above cover `assemble`, which can sweep once at the end because it
// knows the whole set up front. `/reload` cannot: `registry::assemble` sweeps
// there too, and then `register_compaction` registers a handle into the same live
// registry - after the sweep, with no second one. That is #266's shape ("the
// decision did not precede the registration") in a second place, which is why the
// brief asks for the sibling sweep.
//
// The gate is `registry::register_gated`: register, then apply the predicate to
// that one name immediately. These rows drive it through the real
// `CompactionDefault` handle, so they exercise the code the `/reload` path runs.

// Verifies: gh #266's seam - **a registration into a live registry is gated
// regardless of when it happens.**
//
// The setup matters: the registry is **fresh** and the handle has never been
// disabled, so the only thing that can turn it off is the gate running during the
// registration. My first version of this row pre-disabled the name and then
// registered - which passed even with the gate gutted, because the disablement was
// already in place. The distinction is the whole point: `set_enabled(false)` then
// register proves nothing, while register-then-gate proves the seam.
#[test]
#[cfg(feature = "bundled-compaction-default")] // `compaction_handle_for_tests` needs the bundled extension.
fn a_handle_registered_after_the_sweep_is_still_gated() {
    let sandbox = sandbox("mcp-late-registration");
    let mut registry = lca_core::ExtensionRegistry::new();

    lca_cli::registry::register_gated(
        &mut registry,
        lca_cli::registry::compaction_handle_for_tests(&sandbox.state_dir()),
        &|name| name == "compaction-default",
    );
    assert!(
        !registry.is_enabled("compaction-default"),
        "the gate applied during the registration, with no later sweep to help it"
    );
    // The handle is still *present* - disabled means inert, not deleted - which is
    // what keeps `/reload` able to re-enable it in place.
    assert!(
        registry
            .registered_names()
            .iter()
            .any(|name| name == "compaction-default"),
        "a gated registration is registered, just disabled"
    );
}

// Verifies: gh #266's seam - **the gate is a filter, not a blanket disable.**
// A predicate that allows the name must leave it enabled, or the fix would pass
// the row above by disabling everything.
#[test]
#[cfg(feature = "bundled-compaction-default")] // `compaction_handle_for_tests` needs the bundled extension.
fn the_gate_leaves_a_permitted_registration_enabled() {
    let sandbox = sandbox("mcp-late-registration-allowed");
    let mut registry = lca_core::ExtensionRegistry::new();
    lca_cli::registry::register_gated(
        &mut registry,
        lca_cli::registry::compaction_handle_for_tests(&sandbox.state_dir()),
        &|_name| false,
    );
    assert!(
        registry.is_enabled("compaction-default"),
        "a permitted registration stays enabled"
    );
}

// Verifies: gh #266 - **`register_compaction` goes through the seam**, so the
// `/reload` and `Ui::new` paths cannot register an ungated handle. Asserted on the
// call site: the behavioural rows above would still pass if `register_compaction`
// kept calling `registry.register` directly while only the test used the gate.
#[test]
#[cfg(feature = "bundled-compaction-default")] // `compaction_handle_for_tests` needs the bundled extension.
fn register_compaction_registers_through_the_gate() {
    let source =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/registry.rs"))
            .expect("read registry.rs");
    let compaction = source
        .split("pub(crate) fn register_compaction(")
        .nth(1)
        .and_then(|rest| rest.split("/// No bundled compaction").next())
        .expect("register_compaction exists");
    assert!(
        compaction.contains("register_gated("),
        "the compaction registration applies enablement itself:\n{compaction}"
    );
    assert!(
        !compaction.contains("registry.register("),
        "and does not bypass the gate with a bare registration:\n{compaction}"
    );
}
