//! What the interface says about a terminal that refused raw mode (gh #592).
//!
//! # The defect this closes
//!
//! gh #323 defect 3 stopped a raw-mode refusal from vanishing into a `.ok()`:
//! the console's two `SetConsoleMode` returns are now checked, and the reason
//! travels out through `Terminal::raw_mode_error()`. Nothing ever read it.
//! Measured on this tree before this file existed:
//!
//! ```text
//! $ grep -rn "raw_mode_error()" --include=*.rs .   # only the trait default and its impl
//! ```
//!
//! So a terminal that refused the mode change left the app running on **cooked**
//! input with no explanation at all: keys echo into the frame, `Ctrl+C` is the
//! shell's rather than LCA's, and every key LCA promises to interpret is handled
//! by somebody else. The trait's own doc says the report exists because routing
//! it "is still owed" - this is that routing, and it is a *notice*, not an
//! error: the interface is usable, just degraded, and the notice says which
//! keys changed meaning instead of refusing to start.
//!
//! # Why a function and not an `UiError` variant
//!
//! gh #575 asked for a typed error at `run`'s boundary. Measured: `run` is
//! **total** - 371 lines with zero `?` and zero `Err(`, every exit an `Ok`. The
//! only failure it could ever report is this one, and it is not fatal, so it
//! belongs in the startup notices beside the keybinding report rather than in
//! an `Err` the caller would print and turn into exit 1. `run`'s return type
//! therefore stays `Result<i32, UiError>`-shaped *information* (`i32`) with no
//! error half to construct - see the report for the full refutation.

use lca_tui::engine::terminal::Terminal;

/// The notice for a terminal that could not enter raw mode, or `None` when it
/// did (gh #592).
///
/// Named "warning: " like every other degraded-startup notice (the shell
/// resolution failure, the tool-selection typo), and it says what a user loses
/// rather than what the subsystem called the failure: keys reach the shell's
/// line discipline, so the interface cannot interpret them.
#[must_use]
pub(crate) fn cooked_input_notice(terminal: &dyn Terminal) -> Option<String> {
    let reason = terminal.raw_mode_error()?;
    Some(format!(
        "warning: this terminal refused raw mode ({reason}); keys are read by the \
         shell, so LCA's keybindings, the editor, and Ctrl+C will not work here - \
         run it in a terminal that permits raw mode"
    ))
}

#[cfg(test)]
#[path = "run_terminal_cooked_tests.rs"]
mod tests;
