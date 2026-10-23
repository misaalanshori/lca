//! gh #463 - a scan must not warn about a directory that is not what it thinks.
//!
//! Launching `lca` from `$HOME` printed, for **every** globally installed
//! extension:
//!
//! ```text
//! warning: skipping project extension `antigravity`: no component.wasm
//! ```
//!
//! and then loaded the extension successfully. Wrong twice: it is not a project
//! extension, and it was not skipped.
//!
//! The mechanism is path overlap. The project-extension directory is
//! `<cwd>/.lca/extensions` and the user data directory is `~/.lca/extensions`
//! (FR-SESS-8: everything lives in `$HOME/.lca` on every platform). When
//! `cwd == $HOME` they are the same place, so the scan read the **install tree** -
//! whose components are named `sha256-<digest>.wasm`, never `component.wasm` - and
//! concluded every installed extension was a broken project one.
//!
//! The general rule this guard stands for: **a scan that finds "no component.wasm"
//! in the data home means the scan is looking in the wrong place**, and reporting
//! that as a user-facing warning blames the user for the code's mistake. If the
//! loader cannot tell the two cases apart, saying nothing is better than saying
//! something false.
//!
//! Three things must stay true:
//!
//! 1. **The comparison is canonical**, not a string prefix. The third cycle in a
//!    row where a raw comparison passed on Linux and failed elsewhere (#436's
//!    registry keys, #389's git fixtures) - so this is pinned structurally.
//! 2. **The genuine diagnostic survives.** A real `.lca/extensions/<name>/` in a
//!    real project, missing its component, still warns.
//! 3. **The same confusion is closed on the trust prompt.** From `$HOME`,
//!    installed extensions made the TUI offer "Trust this project folder?", which
//!    is the same root error through a second path.
//!
//! Behaviour rows live in `crates/lca-cli/src/project_ext.rs::tests`
//! (`no_project_scan_when_cwd_is_the_data_home`,
//! `a_real_broken_project_extension_still_warns`,
//! `a_symlinked_data_home_is_still_recognised`) and the existing
//! `project_extension_layout_scans_valid_dirs_only` pin.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #463 - the scan takes the data directory and skips when the two
// directories are the same. A `scan_project_extensions(cwd)` that had no way to
// know about the data home is the shape that made the bug possible.
#[test]
fn the_scan_knows_where_the_data_home_is() {
    let source = read("crates/lca-cli/src/project_ext.rs");
    assert!(
        source.contains("fn scan_project_extensions(\n    cwd: &std::path::Path,\n    data_dir: &std::path::Path,"),
        "the scan is told the data home, so it can tell the cases apart"
    );
    assert!(
        source.contains("same_path(&dir, &data_dir.join(\"extensions\"))"),
        "and the skip is keyed on the two directories being the same place"
    );
}

// Verifies: gh #463 design point 1 - **canonical comparison**, not a string
// prefix.
//
// This is the row that would fail if `same_path` were replaced by
// `left == right` on the raw paths: a `$HOME` that is a symlink, macOS's
// `/var` -> `/private/var`, and Windows's `\\?\` verbatim prefix all make the
// string comparison agree on one platform and not another.
#[test]
fn the_comparison_resolves_symlinks() {
    let source = read("crates/lca-cli/src/project_ext.rs");
    let start = source
        .find("pub(crate) fn same_path(")
        .expect("the comparison helper exists");
    let body = &source[start..];
    let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
    // The call itself, not the doc comment: the comment names the mistake.
    let code: String = body
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.contains("canonicalize(left)") && code.contains("canonicalize(right)"),
        "both sides are canonicalized: {code}"
    );
    assert!(
        !code.contains("starts_with") && !code.contains("to_string_lossy"),
        "and not a lexical spelling comparison: {code}"
    );
}

// Verifies: gh #463 design point 3 - the diagnostic cannot be disabled by
// accident. The genuine case names the extension and the reason.
#[test]
fn the_genuine_warning_is_still_emitted() {
    let source = read("crates/lca-cli/src/project_ext.rs");
    assert!(
        source.contains("skipping project extension `{}`: no component.wasm"),
        "a real broken project extension still warns, with its name and reason"
    );
    let tests = &source[source.find("mod tests").unwrap_or(0)..];
    assert!(
        tests.contains("fn a_real_broken_project_extension_still_warns"),
        "and a row pins that it does"
    );
}

// Verifies: gh #463 - the **second path** to the same confusion: the project-trust
// prompt.
//
// From `$HOME` with only installed extensions, the TUI offered "Trust this
// project folder?" because `cwd/.lca/extensions` held entries. Measured live: the
// prompt appeared, and disappeared once the check was added.
#[test]
fn the_trust_prompt_uses_the_same_check() {
    let hooks = read("crates/lca-cli/src/tui/hooks.rs");
    assert!(
        hooks.contains(
            "fn project_extensions_wait(cwd: &std::path::Path, data_dir: &std::path::Path)"
        ),
        "the prompt's predicate is told the data home too"
    );
    assert!(
        hooks.contains("crate::project_ext::same_path"),
        "and reuses the one comparison, so the two paths cannot drift apart"
    );
    assert!(
        hooks.contains("project_config_is_the_user_file"),
        "a project config file that is the user's own must not trigger a trust prompt"
    );
}

// Verifies: gh #463 - no new dependency; the comparison is `std::fs::canonicalize`
// plus `==`.
#[test]
fn no_new_dependency_was_added() {
    let manifest = read("crates/lca-cli/Cargo.toml");
    for unwanted in ["pathdiff", "same-file", "dunce"] {
        assert!(
            !manifest.contains(&format!("\n{unwanted} =")),
            "no {unwanted} dependency for a canonical comparison"
        );
    }
}
