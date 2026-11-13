//! Gh #544 - the CLI's output DTOs are closed to outside literals, and **no
//! production path branches on an error message**.
//!
//! The #549 half (the turn-step enum, its zero suppressions, and `AttachmentError`)
//! is pinned behaviourally in `crates/lca-core/tests/turn_step.rs`; this row is the
//! CLI half of the batch, so the two issues that share one commit have one file
//! each in their own crate plus this structural pair here.
//!
//! Both halves were measured before anything was changed, and one of the issue's
//! claims is refuted:
//!
//! * **`#[non_exhaustive]` on eleven DTOs** (`ContextFile`, `AppendInstruction`,
//!   `PromptFiles`, `PromptInputs`, `ToolSelection`, `PlannedMessages`, `Plan`,
//!   `Written`, `ResolvedModel`, `Invocation`, `UpdatePlan`): **zero** external
//!   struct literals for every one of them (`grep` over `crates/`, `tests/`,
//!   `extensions/`), so closing them breaks no caller.
//! * **"replace `String` errors"**: the CLI has **no branch-on-message
//!   boundary**. Outside `#[cfg(test)]` modules and `*_tests.rs`, the only
//!   `contains("")` in `crates/lca-cli/src/` is `entry.source.contains("@sha256:")`
//!   in `ext/manage.rs` - a check on a *registry reference*, not on an error. Every
//!   `String`-returning helper's refusal is printed (`eprintln!("{err}")`) and
//!   mapped to an exit code; `tui::run` and `trust::load_config` already return
//!   `anyhow::Result`, which is the documented spelling for the binary crate.
//!   So converting those helpers would be churn, and it is **declined by count**.
//! * **The sleeps**: three of the four are already bounded (`tmux_doctor.rs`'s two
//!   5 ms polls sit inside 250/500 ms deadlines; `startup.rs`'s one is behind
//!   `LCA_STARTUP_STALL_MS`, an opt-in stall knob clamped to 10 s), and the 80 ms
//!   one at `tmux_doctor.rs:520` is **inside `#[cfg(test)]`** - so the claim that
//!   they delay exit on `Ctrl+C` is refuted too.
//!
//! Verifies: gh #544 acceptance 1-4.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo().join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #544 - the eleven output DTOs are closed, each at its real home.
#[test]
fn the_cli_output_dtos_are_closed_to_outside_literals() {
    for (path, name) in [
        ("crates/lca-cli/src/prompt.rs", "ContextFile"),
        ("crates/lca-cli/src/prompt.rs", "AppendInstruction"),
        ("crates/lca-cli/src/prompt.rs", "PromptFiles"),
        ("crates/lca-cli/src/prompt.rs", "PromptInputs"),
        ("crates/lca-cli/src/invoke.rs", "ToolSelection"),
        ("crates/lca-cli/src/invoke.rs", "PlannedMessages"),
        ("crates/lca-cli/src/bug.rs", "Plan"),
        ("crates/lca-cli/src/bug.rs", "Written"),
        ("crates/lca-cli/src/models.rs", "ResolvedModel"),
        ("crates/lca-cli/src/session_cmds/mod.rs", "Invocation"),
        ("crates/lca-cli/src/update.rs", "UpdatePlan"),
    ] {
        let source = read(path);
        let closed = source.contains(&format!("#[non_exhaustive]\npub struct {name} {{"))
            || source.contains(&format!("#[non_exhaustive]\npub struct {name}<'a> {{"));
        assert!(
            closed,
            "`{name}` in {path} is `#[non_exhaustive]`: measured zero external literals, \
             so closing it breaks no caller and a field added later breaks none either"
        );
    }
    // And the ones the issue says to keep exhaustive still are (clap derive /
    // constructed by callers).
    let cli_args = read("crates/lca-cli/src/cli_args.rs");
    for open in ["pub struct Cli {", "pub struct CliFlags {"] {
        assert!(
            cli_args.contains(open),
            "`{open}` stays exhaustive: clap derives it, and flags are built field by field"
        );
    }
}

// Verifies: gh #544 - **no production path branches on an error message**, which
// is what makes most of the issue's "replace `String` errors" cosmetic. The row
// reads the crate's own source and fails if a `contains` appears outside a test
// module (one legitimate data check excepted, named below).
#[test]
fn no_production_path_branches_on_an_error_message() {
    let mut offenders = Vec::new();
    for (path, text) in crate::module_sources("crates/lca-cli/src/lib.rs") {
        // Everything from the first `#[cfg(test)]` onward is test code, and the
        // `*_tests.rs` files are test modules by name.
        let production = match text.find("#[cfg(test)]") {
            Some(index) => &text[..index],
            None => &text[..],
        };
        if path.ends_with("_tests.rs") || path.ends_with("/tests.rs") || path.contains("/tests/") {
            continue;
        }
        for (index, line) in production.lines().enumerate() {
            if !line.contains("contains(\"") || line.trim_start().starts_with("//") {
                continue;
            }
            // The one legitimate occurrence: a **data** check on a registry
            // reference (`host/repo/name@sha256:...`), not on an error message.
            if line.contains("source.contains(\"@sha256:\")") {
                continue;
            }
            offenders.push(format!("{path}:{}: {}", index + 1, line.trim()));
        }
    }
    assert!(
        offenders.is_empty(),
        "a production path branches on a message; that is the one case a typed error \
         would fix (otherwise the `String` is only printed, and converting it is churn):\n{}",
        offenders.join("\n")
    );
}

// Verifies: gh #544 - the two named boundaries already return the binary crate's
// documented error type, and both callers print-and-map rather than inspect.
#[test]
fn the_cli_boundaries_use_the_binarys_error_type() {
    let tui = read("crates/lca-cli/src/tui/mod.rs");
    assert!(
        tui.contains(") -> anyhow::Result<i32> {"),
        "`tui::run` returns the binary crate's error type; `anyhow` is allowed here \
         by the coding standards and the value it carries is a message + exit code"
    );
    let trust = read("crates/lca-cli/src/trust.rs");
    assert!(
        trust.contains(") -> anyhow::Result<Config> {"),
        "`trust::load_config` and `load_config_flags` do the same"
    );
    // The consumer prints and maps to an exit code, which is the documented
    // shape for a CLI's outermost error (`docs/headless.md`'s codes).
    let invoke = read("crates/lca-cli/src/invoke.rs");
    assert!(
        invoke.contains("eprintln!(\"error: {err:#}\")")
            && invoke.contains("crate::exit::INTERNAL"),
        "the outermost consumer prints the message and maps it to an exit code"
    );
}
