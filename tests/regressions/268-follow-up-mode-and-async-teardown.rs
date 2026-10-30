//! Gh #268: RPC ignored `queue.follow_up_mode`, and headless spawned a runtime
//! inside `async fn`.
//!
//! Both defects are the same shape - **the third caller was not updated** - so
//! both guards here are about the *seam* rather than the instance.
//!
//! # The ordering rule
//!
//! `queue.follow_up_mode` is documented (`docs/configuration.md`) and honoured by
//! TUI (`tui/commands.rs`) and by `lca-ui`'s queue, but `rpc.rs` drained its
//! follow-up list one at a time unconditionally. An embedder driving LCA over RPC
//! therefore saw different semantics from the same session in a terminal.
//!
//! The behavioural rows (`crates/lca-cli/tests/e2e_rpc.rs`) assert what the
//! provider received under each mode. This file pins **where the decision lives**
//! and **how many surfaces honour it**, because a green suite cannot see a fourth
//! surface inventing a third answer.
//!
//! # The teardown
//!
//! `headless` and `rpc::finish` called `lca_core::drive_blocking(async { …
//! on_session_close().await … })` from inside an `async fn`: an OS thread plus a
//! single-thread Tokio runtime inside a running async context, for a hook that is
//! already `async fn`. `.await` is the fix; these rows keep the idiom from
//! coming back to an async call site.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

/// The body of one function, found by brace matching from its signature.
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut seen = false;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                depth += 1;
                seen = true;
            }
            b'}' => {
                depth -= 1;
                if seen && depth == 0 {
                    return &source[start..=i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("{signature} has a balanced body");
}

/// Source with `//` comment lines removed, so a row about *code* is not satisfied
/// or defeated by prose. (Two of my guards in earlier cycles matched their own
/// explanatory comments.)
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Verifies: gh #268 - the RPC drain branches on the configured mode.
//
// The defect was that `follow_ups.remove(0)` ran unconditionally. The guard is
// that both `QueueMode` arms appear in the drain, and that `All` joins - the same
// `"\n\n"` separator `lca-ui`'s `take_all_pending` uses, so the two surfaces
// cannot drift into different join rules.
#[test]
fn the_rpc_drain_honours_both_queue_modes() {
    let source = code_only(&read("crates/lca-cli/src/rpc.rs"));
    let body = function_body(&source, "pub async fn rpc(");

    assert!(
        body.contains("QueueMode::All"),
        "the `all` arm exists in the drain: no arm means the setting is ignored"
    );
    assert!(
        body.contains("QueueMode::OneAtATime"),
        "and the serial arm is named too, so neither is an accident of the default"
    );
    assert!(
        body.contains(r#".join("\n\n")"#),
        "`all` joins the queued messages, as `lca-ui`'s `take_all_pending` does"
    );
    // The unconditional drain that caused the divergence must be gone: the only
    // `remove(0)` is inside the serial arm.
    let removes = body.matches("follow_ups.remove(0)").count();
    assert_eq!(
        removes, 1,
        "exactly one `remove(0)`, the serial arm - a second would be an \
         unconditional drain: {removes}"
    );
}

// Verifies: gh #268 - the mode is resolved **once**, where config is read.
//
// The seam the brief asked about: three surfaces honour the setting now, and a
// fourth must not parse it differently. `setup()` resolves it into `Setup`, so
// there is one parse and one default for the whole product; TUI keeps its own
// call because it takes the config directly rather than a `Setup`.
#[test]
fn the_mode_is_resolved_once_where_the_config_is_loaded() {
    let source = code_only(&read("crates/lca-cli/src/headless.rs"));
    let setup = function_body(&source, "pub(crate) async fn setup(");
    assert!(
        setup.contains("QueueMode::parse(config.queue_follow_up_mode())"),
        "setup resolves the documented key: {setup}"
    );
    assert!(
        setup.contains("unwrap_or_default()"),
        "and falls back to the documented default, matching TUI's parse"
    );
    assert!(
        setup.contains("follow_up_mode"),
        "carrying it out on `Setup` for the RPC loop to use"
    );
    // TUI still parses the same key with the same fallback - two surfaces, one rule.
    let tui = code_only(&read("crates/lca-cli/src/tui/commands.rs"));
    assert!(
        tui.contains("QueueMode::parse(config.queue_follow_up_mode())"),
        "TUI parses the same key the same way"
    );
    assert!(tui.contains("unwrap_or_default()"), "with the same default");
}

// Verifies: gh #268 - no `drive_blocking` remains at an async call site.
//
// This is the sweep, expressed as a guard: the hazard is
// `drive_blocking` **inside an `async fn`**, which spawns a thread and a nested
// runtime in a context that already has one. The legitimate uses are all in sync
// functions on the interface's command thread, where a nested `block_on` would
// panic - so the guard must not ban the helper, only its async misuse.
#[test]
fn the_teardown_is_awaited_not_bridged_from_async() {
    for (file, signature) in [
        ("crates/lca-cli/src/headless.rs", "pub async fn headless("),
        ("crates/lca-cli/src/rpc.rs", "async fn finish("),
    ] {
        let source = code_only(&read(file));
        let body = function_body(&source, signature);
        assert!(
            !body.contains("drive_blocking"),
            "{file}: {signature} is async, so the teardown is awaited rather than \
             bridged onto a fresh thread + runtime"
        );
        assert!(
            body.contains("on_session_close().await"),
            "{file}: and the hook is awaited to completion, not fire-and-forget"
        );
        // The teardown must still precede the session close: an async fn that is
        // dropped mid-await would otherwise let the store close first.
        let teardown = body.find("on_session_close().await").expect("the teardown");
        let close = body
            .find("store.close")
            .unwrap_or_else(|| panic!("{file}: the session is closed"));
        assert!(
            teardown < close,
            "{file}: the teardown completes before the session is closed"
        );
    }
}

// Verifies: gh #268 - every *remaining* `drive_blocking` call sits in a sync fn.
//
// The sweep's answer, kept as a row so the next async caller is caught: the
// interface's command thread is the only place the bridge belongs, because a
// nested `block_on` there would panic. A new async-context call site fails here.
#[test]
fn every_remaining_drive_blocking_call_is_in_a_synchronous_function() {
    let root = repo_root().join("crates");
    let mut offenders: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for entry in walk(&root) {
        let relative = entry.strip_prefix(&root).unwrap_or(&entry).to_path_buf();
        if relative.components().any(|c| c.as_os_str() == "tests") {
            continue;
        }
        let text = std::fs::read_to_string(&entry).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !line.contains("drive_blocking") || line.trim_start().starts_with("//") {
                continue;
            }
            // Walk back to the enclosing `fn`.
            for back in (0..=index).rev() {
                let candidate = lines[back];
                if candidate.trim_start().starts_with("//") {
                    continue;
                }
                let Some(fn_at) = candidate.find("fn ") else {
                    continue;
                };
                // Skip a `fn` whose name is on a later line (rare here).
                let head = &candidate[..fn_at];
                checked += 1;
                if head.contains("async") {
                    offenders.push(format!(
                        "{}:{} in an async fn: {}",
                        relative.display(),
                        index + 1,
                        candidate.trim()
                    ));
                }
                break;
            }
        }
    }
    assert!(
        checked >= 10,
        "the sweep found the call sites: {checked} (a silent zero would mean the \
         walker broke)"
    );
    assert!(
        offenders.is_empty(),
        "`drive_blocking` inside an `async fn` builds a thread and a nested \
         runtime in a context that already has one: {offenders:#?}"
    );
}

/// Every `.rs` file under `dir`, recursively.
fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out
}
