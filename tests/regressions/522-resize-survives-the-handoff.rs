//! NFR-24 guard for gh #522: a resize during a tty handoff reaches the layout.
//!
//! ## The defect
//!
//! The `$EDITOR` prompt handoff (and its two siblings, an interactive `!` command
//! and `Ctrl+Z` suspend) is `screen.leave` → `terminal.stop()` → run the child →
//! `terminal.start()`. `stop()` joins the reader thread; `start()` spawns a fresh
//! one, and that reader seeds its own `last_size` from `sys::terminal_size()`. So a
//! resize that lands while the child owns the tty is compared against **nothing**: it
//! is not a change from the new reader's point of view, no resize fires, and the
//! interface composes its next frame at the width recorded before the handoff.
//!
//! Measured in a real pane (140 → 60 during an `$EDITOR` nap), before the fix: three
//! trials of three left the resumed frame with a 140-cell editor-frame cap wrapped by
//! tmux into `╭───` plus a continuation row. The control - the same resize with no
//! handoff - was clean 5/5, so the handoff window is the cause and not the resize path.
//!
//! ## What this pins
//!
//! Behaviour, at two levels, plus the blocking rule that made the defect invisible
//! for a cycle:
//!
//! - `lca-tui`'s `a_resumed_terminal_adopts_the_size_the_tty_has_now` and
//!   `adopting_the_size_before_the_reader_starts_does_not_double_notify`: the rule
//!   itself, on the same `adopt_size` the product calls (no tty needed, so every
//!   platform runs it).
//! - `lca-ui`'s `the_frame_is_laid_out_for_the_width_it_is_painted_into`: the frame's
//!   caps span exactly the width the caller passes, with `world.size` deliberately set
//!   to a *different* width - the stale-copy class the brief names.
//! - `lca-cli`'s `e2e_resize_handoff`: the real pane row. Red on the pre-fix tree
//!   ("the widest painted line is still 140 cells"), green after.
//! - `harness_contract`'s `a_wrapped_box_cap_is_measured_at_the_width_it_was_laid_out`:
//!   the instrument. A wrapped cap's physical rows all *fit* the pane, so the old
//!   max-width rule called a 140-wide layout healthy at 60 columns; `logical_width` now
//!   joins a cap whose corner went missing and reports the laid-out width. Red with the
//!   old rule (measured 60), green now (measured 142).
//!
//! What is left for this file is the one claim a behaviour row cannot state: **the
//! restart is where the size is re-derived, and it is re-derived from the tty itself.**
//! A future "fix" that remembered a size, or that adopted it only on one of the three
//! handoff paths, would pass the pane row on a slow machine and leave the others stale.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use std::path::Path;

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|err| panic!("read {rel}: {err}"))
}

const TERMINAL: &str = "crates/lca-tui/src/engine/terminal.rs";
const READER: &str = "crates/lca-tui/src/engine/reader_loop.rs";

/// The body of one function, from its `fn` line to the closing `    }` at method
/// indentation.
fn body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` exists in {TERMINAL}"));
    let rest = &source[start..];
    let end = rest
        .find("\n    }\n")
        .unwrap_or_else(|| panic!("`{signature}` ends with an indented `}}`"));
    &rest[..end]
}

// Verifies: gh #522 - `start` adopts the tty's current size, and does it through the
// one helper every handoff path shares.
//
// `start` is the seam all three handoffs resume through (`$EDITOR`, an interactive
// `!`, a suspend's `restart_reader`), so the adoption lives there rather than in each
// caller. Asserting the *call* rather than the mechanism is deliberate: a fix that
// re-derived the size in `run_terminal::restart_reader` only would leave the `$EDITOR`
// path (which calls `terminal.start` directly) stale.
#[test]
fn the_terminal_start_adopts_the_size_the_tty_has_now() {
    let source = read(TERMINAL);
    let start = body(
        &source,
        "    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler) {",
    );
    assert!(
        start.contains("self.adopt_current_size();"),
        "a start (a resume after a handoff) re-derives the size from the tty: {start}"
    );
    // Order matters: the handlers must be installed *before* the notification, or the
    // fire reaches the previous frame's no-op handler and the loop never hears it.
    let install_input = start
        .find("*h = on_input;")
        .expect("the input handler is installed");
    let install_resize = start
        .find("*h = on_resize;")
        .expect("the resize handler is installed");
    let adopt = start
        .find("self.adopt_current_size();")
        .expect("checked above");
    assert!(
        install_input < adopt && install_resize < adopt,
        "the handlers are installed before the adoption fires: input={install_input} \
         resize={install_resize} adopt={adopt}"
    );
}

// Verifies: gh #522 - the adoption reads the window size and not a remembered value.
//
// The defect was a stale *copy* of one fact. The rule is therefore "ask the tty", and
// the row holds that: `adopt_current_size` calls `sys::terminal_size()`, and
// `adopt_size` compares against the stored size before notifying (so a resume that
// finds nothing changed stays quiet instead of repainting every `$EDITOR` exit).
#[test]
fn the_adoption_asks_the_tty_and_notifies_only_on_a_change() {
    let source = read(TERMINAL);
    let current = body(&source, "    fn adopt_current_size(&self) {");
    assert!(
        current.contains("sys::terminal_size()"),
        "the size comes from the terminal, never from a remembered value: {current}"
    );
    let adopt = body(
        &source,
        "    pub(crate) fn adopt_size(&self, size: (u16, u16)) {",
    );
    assert!(
        adopt.contains("fire_resize()"),
        "a moved size notifies the loop, so the next frame is tier 2: {adopt}"
    );
    // The comparison is against the live cells, and the notification sits behind it.
    let compares = adopt
        .find("Ordering::SeqCst)")
        .expect("the stored size is read");
    let fires = adopt.find("fire_resize()").expect("checked above");
    assert!(
        compares < fires,
        "the notification is behind the comparison (an unchanged size must stay quiet): \
         compare={compares} fire={fires}"
    );
    // And the reader's own baseline cannot double-notify: it seeds `last_size` from
    // the same ioctl after the adoption, so the first iteration already agrees.
    let reader_source = read(READER);
    let reader = body(&reader_source, "pub(crate) fn reader_loop(");
    assert!(
        reader.contains("let mut last_size = sys::terminal_size()"),
        "the reader seeds its baseline from the same source: {reader}"
    );
}

// Verifies: gh #522 - the harness measures a wrapped cap, not just padding.
//
// The instrument half is what let the defect hide: every physical row of a wrapped
// 140-cell cap fits a 60-column pane, so a max-width rule answered "60, fine".
// `logical_width` now joins a cap whose corner went missing and reports the width the
// app laid out, and `harness_contract` pins both directions (the fixture row above and
// the pre-existing 140-wide padding row, which must still fail).
#[test]
fn the_harness_measures_a_cap_whose_corner_wrapped_away() {
    let readiness = read("crates/lca-cli/tests/common/readiness.rs");
    let width = body(&readiness, "    pub fn logical_width(&self) -> usize {");
    assert!(
        width.contains("'╭'") && width.contains("'╮'"),
        "the measurement joins a cap whose corner wrapped onto the next row: {width}"
    );
    assert!(
        width.contains("'╰'") && width.contains("'╯'"),
        "and the editor frame's floor, which is the row the pane row actually saw: {width}"
    );
    // The padding rule stays: a genuinely over-wide layout made of padded rows must
    // still be caught, which is the pre-existing contract row's negative direction.
    assert!(
        width.contains("physical * longest_run"),
        "the padding-run rule is unchanged, so the old negative row still bites: {width}"
    );
}
