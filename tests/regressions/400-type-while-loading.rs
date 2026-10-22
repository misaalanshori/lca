//! GitHub #400 (type-while-loading slice): typing works while startup finishes.
//!
//! # The defect, as measured rather than described
//!
//! `lca` runs a lot before the interactive loop exists: the grant store, the
//! merged config, the session resolve, then `Ui::new` loads the extension
//! registry. The terminal only entered raw mode and started reading *inside*
//! `lca_ui::run`, after all of it. Measured on this machine: ~20 ms between
//! process start and the loop, of which the load is 17-20 ms - short, but it
//! grows with what is installed, which is the wrong scaling (the users with the
//! most extensions get the worst startup).
//!
//! What that window did, observed in a real pane:
//!
//! - **Printed text was held by the tty** and arrived late, in one lump after
//!   the header - late, not lost.
//! - **A quit key was inert.** `Ctrl+C` before the loop did nothing: the shell's
//!   line discipline had it, and nothing was listening. A user pressing it to
//!   leave a slow startup had to wait, then press again (and again: gh #225's
//!   armed-exit needs two presses *after* the loop, so the first press was
//!   silently spent).
//!
//! # The fix
//!
//! Claim the terminal before the load: raw mode and the reader go live at ~3 ms,
//! buffering into [`EarlyInput`]. The loop adopts that same terminal and drains
//! the same receiver, so there is no handover window and no second reader. Keys
//! buffered before the loop are replayed through the loop's own handler, in
//! arrival order, so an early key means exactly what a late one does.
//!
//! # The three spellings trap (found live, not by any unit test)
//!
//! `start()` negotiates the Kitty keyboard protocol, so `tmux` delivers `Ctrl+C`
//! as `\x1b[99;5u`, not `\u{3}`. A buffer matching only the raw byte held the
//! sequence, recognised nothing, and the quit did nothing - the fix appearing
//! not to work while every unit test passed. `is_quit` now knows all three
//! spellings the keybindings engine knows, with a row for each.
//!
//! The behavioural rows live beside the code (`run_early_input.rs`'s `tests`).
//! This is the NFR-24 named guard: it fails if the mechanism is quietly
//! reverted - input gated on loading, the buffer dropped, or the quit path
//! narrowed back to one spelling.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

// Verifies: gh #400 - **the terminal is claimed before the load**, in the host's
// startup path. If `EarlyInput::start()` moves back after `Ui::new`, the window
// reopens and nothing else in this feature matters.
#[test]
fn the_terminal_is_claimed_before_the_load() {
    let module = read("crates/lca-cli/src/tui/mod.rs");
    // The claim itself lives in `startup::claim_terminal` (moved there when
    // `mod.rs` crossed the 1,200-line ceiling); the ordering is what matters,
    // so the guard follows the call, not the definition.
    let claim = module
        .find("startup::claim_terminal()")
        .expect("the host claims the terminal at startup");
    let startup = read("crates/lca-cli/src/tui/startup.rs");
    assert!(
        startup.contains("EarlyInput::start()"),
        "and that call really starts the early reader"
    );
    let load = module
        .find("Ui::new(")
        .expect("the host loads the registry in `Ui::new`");
    assert!(
        claim < load,
        "the claim must come before the load: claiming afterwards is the \
         defect this feature exists to fix"
    );
    assert!(
        module.contains("quit_requested_during_startup()"),
        "the startup path checkpoints for a quit instead of ignoring it"
    );
}

// Verifies: gh #400 - **the loop adopts the claimed terminal** rather than
// starting a second one. A second reader would leave the buffered keys owned by
// a terminal nobody reads, which is how the first attempt silently dropped text.
#[test]
fn the_loop_adopts_the_claimed_terminal() {
    let run = read("crates/lca-ui/src/run.rs");
    assert!(
        run.contains("options.early_input.take()"),
        "the loop takes the claimed terminal"
    );
    assert!(
        run.contains("AdoptedTerminal::take("),
        "the adoption is one named step, not inline control flow"
    );
    // The adoption step itself lives beside the buffer (it was inlined in
    // `run.rs` until the ceiling split), so the guard reads both files.
    let adopted = read("crates/lca-ui/src/run_early_input.rs");
    // Formatting-independent: `cargo fmt` reflows the struct literal, so these
    // are matched separately rather than as one exact line.
    assert!(
        adopted.contains("early.into_loop()"),
        "and it reads the same receiver the buffer read from - one stream, no gap"
    );
    assert!(
        adopted.contains("receiver: Some("),
        "the adopted receiver is kept, not dropped and remade"
    );
    assert!(
        adopted.contains("pub struct EarlyStream"),
        "the stream carries both halves, so a `$EDITOR` restart can re-point the \
         reader at the channel the loop drains"
    );
    let early = read("crates/lca-ui/src/run_early_input.rs");
    assert!(
        early.contains("The terminal is **not** restarted."),
        "why the reader is not restarted is documented where it is decided"
    );
}

// Verifies: gh #400 - **nothing is dropped**, and order is kept. The buffer is a
// list replayed in arrival order, not a set and not a string rebuild.
#[test]
fn buffered_input_is_ordered_and_replayed() {
    let early = read("crates/lca-ui/src/run_early_input.rs");
    assert!(
        early.contains("pending: Vec<String>"),
        "an ordered list of whole `data` values"
    );
    assert!(
        early.contains("fn drain(&mut self) -> bool"),
        "draining collects without blocking"
    );
    let run = read("crates/lca-ui/src/run.rs");
    assert!(
        run.contains("for data in &startup_keys"),
        "the loop replays them through its own handler"
    );
}

// Verifies: gh #400 (FR-CORE-5's spirit) - **quit acts, and acts in every
// spelling**. The three-spelling row exists because the live failure was
// exactly this: the sequence arrived, nothing recognised it.
#[test]
fn quit_is_recognised_in_every_spelling_the_terminal_uses() {
    let early = read("crates/lca-ui/src/run_early_input.rs");
    for spelling in [
        r#""\u{3}""#,
        r#""\u{4}""#,
        r#""\x1b[99;5u""#,
        r#""\x1b[100;5u""#,
        r#""\x1b[27;5;99~""#,
        r#""\x1b[27;5;100~""#,
    ] {
        assert!(
            early.contains(spelling),
            "the {spelling} spelling is a quit: the terminal starts sending it \
             the moment `start()` negotiates the Kitty protocol"
        );
    }
    assert!(
        early.contains("the keybindings engine already treats all three")
            || early.contains("The keybindings engine already treats all three"),
        "and the reason all three are needed is recorded"
    );
}

// Verifies: gh #400 (FR-UI-10) - **paste stays atomic.** The buffer stores whole
// `data` values, so a bracketed paste cannot be split into characters.
#[test]
fn paste_is_buffered_as_one_segment() {
    let early = read("crates/lca-ui/src/run_early_input.rs");
    assert!(
        early.contains("never their characters") || early.contains("never their\n//!   characters"),
        "the module states the rule: whole values, never characters"
    );
    assert!(
        early.contains("pending.push(data)") || early.contains("self.pending.push(data)"),
        "and the implementation pushes the whole value"
    );
}
