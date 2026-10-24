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

// ===========================================================================
// Gh #400 gaps 2 and 3 (this batch): the compact header and collision notices.
//
// Gap 2's defect was that every launch printed a version line, a `context:` line
// listing every AGENTS.md path, and a counts line - a wall at the top of a session
// nobody asked to read, every single launch. The fix is a compact line with the
// expand key, and the failure modes the pattern is named for (empty sections
// leaving orphaned dividers, unfitted rows staggering the border, the expansion
// resetting on `/new`) are pinned by behaviour rows in
// `crates/lca-cli/tests/startup_header.rs` and
// `crates/lca-ui/src/chat_header_tests.rs`.
//
// Gap 3's defect was different in kind, and that is what these rows guard:
// `ExtensionRegistry::register` **already recorded a `CollisionReport`** and
// **nothing read it**. The fact was known, correct and discarded, so a user who
// installed an extension and found the bundled one answering had no way to learn
// why. The registry's decision must stay the registry's - a notice that changed
// what loads would be a much worse bug than a silent one.
// ===========================================================================

// Verifies: gh #400 gap 3 - **the notice reports the registry's decision, it does
// not make it.** `collision_summary` maps a report without deriving anything: the
// loser is the report's own `extension`, the winner its `winner` field. A notice
// that re-derived either side could disagree with what actually loaded.
#[test]
fn the_collision_notice_reports_rather_than_decides() {
    let startup = read("crates/lca-cli/src/tui/startup.rs");
    let summary = startup
        .split("pub fn collision_summary(")
        .nth(1)
        .and_then(|rest| rest.split("pub fn collision_rows(").next())
        .expect("collision_summary exists");
    assert!(
        summary.contains("report.winner.clone()") && summary.contains("report.extension.clone()"),
        "both sides come from the report's own fields:\n{summary}"
    );
    for invented in ["registered_names()", "DeliveryMode", "entries.iter()"] {
        assert!(
            !summary.contains(invented),
            "the notice does not re-scan the registry (`{invented}`)"
        );
    }
}

// Verifies: gh #400 gap 3 - **the collision rides the launch path.** The header is
// built from `registry.collisions()`; before this batch nothing called it, so the
// report was collected and dropped.
#[test]
fn the_launch_reads_the_registrys_collisions() {
    // The conversion lives in `startup::launch_header` after the gate-11 split, so
    // the guard follows the code rather than a filename - and the call site proves
    // it is actually reached at launch, which a helper alone would not.
    let startup = read("crates/lca-cli/src/tui/startup.rs");
    assert!(
        startup.contains("collision_summary(registry.collisions())"),
        "the header builder converts the registry's recorded collisions"
    );
    let module = read("crates/lca-cli/src/tui/mod.rs");
    assert!(
        module.contains("startup::launch_header("),
        "and the launch calls it"
    );
}

// Verifies: gh #400 gap 3 - **the notice survives `quietStartup`.** A shadowed
// extension is a diagnostic. The quiet layers hide the header; they must not hide
// this, or a user who set `quietStartup = true` never learns why their install is
// not the thing answering.
//
// Asserted on the shape: `startup_header_with` appends the collision rows *after*
// the quiet match, so the two can never be reordered without this failing.
#[test]
fn the_collision_rows_sit_outside_the_quiet_layers() {
    let startup = read("crates/lca-cli/src/tui/startup.rs");
    let with = startup
        .split("pub fn startup_header_with(")
        .nth(1)
        .and_then(|rest| rest.split("/// `1 extension`").next())
        .expect("startup_header_with exists");
    let quiet_at = with.find("match quiet").expect("the quiet match exists");
    let extend_at = with
        .find("lines.extend(collision_rows(")
        .expect("the collision rows are appended");
    assert!(
        extend_at > quiet_at,
        "the append happens after the quiet decision, so quiet cannot swallow it:\n{with}"
    );
}

// Verifies: gh #400 gap 2 - **one key, not two.** The header expands on
// `app.tools.expand`, which the interface already binds for tool cards. A second
// binding for "show me the detail" is exactly the kind of chord a user would never
// find, and the brief forbids it.
#[test]
fn the_header_expands_on_the_existing_key() {
    let chat = read("crates/lca-ui/src/chat.rs");
    let arm = chat
        .split("if self.keybindings.matches(data, \"app.tools.expand\")")
        .nth(1)
        .and_then(|rest| rest.split("app.thinking.toggle").next())
        .expect("the expand arm exists");
    assert!(
        arm.contains("toggle_startup_resources()"),
        "the existing expand key toggles the header:\n{arm}"
    );
    // And no second action was invented for it.
    let keybindings = read("crates/lca-tui/src/engine/keybindings.rs");
    assert!(
        !keybindings.contains("app.header.expand") && !keybindings.contains("header.toggle"),
        "no new binding was added for the header"
    );
}

// Verifies: gh #400 gap 2 - **the empty-section rule is structural**, not a
// judgement the painter makes. Every section helper returns `None`/empty, so a
// caller pushes nothing and no spacer can be left behind. A behaviour row can see
// one input; this sees that the shape makes the bad output unrepresentable.
#[test]
fn the_header_sections_are_optional_by_construction() {
    let startup = read("crates/lca-cli/src/tui/startup.rs");
    assert!(
        startup.contains("fn context_row(context: &[std::path::PathBuf]) -> Option<String>"),
        "the context row is an Option, so an empty section yields nothing to push"
    );
    let resources = startup
        .split("fn resources_row(")
        .nth(1)
        .and_then(|rest| rest.split("pub fn collision_rows(").next())
        .expect("resources_row exists");
    assert!(
        resources.contains("if extensions > 0")
            && resources.contains("if skills > 0")
            && resources.contains("if templates > 0"),
        "and each resource count is named only when non-zero:\n{resources}"
    );
}

// ===========================================================================
// gh #400 gap 4: the tmux keyboard doctor
// ===========================================================================
//
// The doctor's decisions live in `crates/lca-cli/src/tui/tmux_doctor.rs` with
// eight unit rows. This is the NFR-24 named guard: it fails if the *mechanism*
// that makes the doctor safe stops holding, which is four things a behavioural
// row cannot see - that the probe is spawned before the load rather than after
// it, that it never blocks, that it is bounded, and that the format is not
// reported as a defect.

// Verifies: gh #400 gap 4 - **the probe starts before the load, so nothing on the
// first frame waits for it.** Measured: one `tmux show` is 34 ms and a launch
// asks two, while the startup load is 200-300 ms. The ordering is what turns
// 68 ms of subprocess into zero frame latency, so it is asserted on position in
// the function rather than on a timing that a fast machine would hide.
#[test]
fn the_tmux_probe_starts_before_the_load_that_hides_it() {
    let startup = read("crates/lca-cli/src/tui/startup.rs");
    let claim = startup
        .split("pub(super) fn claim_terminal(")
        .nth(1)
        .and_then(|rest| rest.split("/// Load the extension registry").next())
        .expect("claim_terminal exists");
    assert!(
        claim.contains("tmux_doctor::spawn()"),
        "the probe is spawned while the terminal is claimed, before the load:\n{claim}"
    );
    // And the launch actually uses the pair, rather than dropping the cell.
    let module = read("crates/lca-cli/src/tui/mod.rs");
    assert!(
        module.contains("let (early_input, tmux_doctor) = startup::claim_terminal();"),
        "the launch keeps the cell"
    );
    assert!(
        module.contains("startup::launch_header(") && module.contains("&tmux_doctor"),
        "and hands it to the header that composes the expanded section"
    );
}

// Verifies: gh #400 gap 4 - **the notice goes in the header's expanded section,
// never into the compact line.** A diagnostic in the default one-liner prints on
// every launch in the one place the reader did not ask; in scrollback it is in the
// conversation an hour later.
//
// Asserted on **values through the real launch function**, not on statement
// position: an earlier version of this row compared two `find` offsets and passed
// a build that fed the doctor rows into `compact`, because the call site was
// rebinding a variable rather than moving. The launch is cheap to call, so there
// is no reason to accept a proxy for the answer.
#[test]
fn the_doctor_reports_into_the_expanded_header_only() {
    use lca_cli::tui::tmux_doctor::{Cell, Doctor};
    let cell: Cell = std::sync::Arc::new(std::sync::OnceLock::new());
    let _ = cell.set(Doctor::ExtendedKeysOff {
        format: Some("xterm".into()),
    });
    let dir = std::env::temp_dir();
    let registry = lca_core::ExtensionRegistry::new();
    let config = lca_config::Config::default();
    let flags = lca_cli::CliFlags::default();
    let (compact, expanded) =
        lca_cli::tui::startup::launch_header(&dir, &dir, false, &flags, &registry, &config, &cell);

    let compact_text = compact.join("\n");
    assert!(
        !compact_text.contains("extended-keys"),
        "the launch's compact line carries no doctor row: {compact_text}"
    );
    let expanded_text = expanded.join("\n");
    assert!(
        expanded_text.contains("set -g extended-keys on"),
        "the expanded section carries the command: {expanded_text}"
    );

    // And the transcript is not a route: nothing in the launch path pushes it.
    let module = read("crates/lca-cli/src/tui/mod.rs");
    assert!(
        !module.contains("push_notice(tmux_doctor") && !module.contains("note_tmux"),
        "no transcript path carries it"
    );
}

// Verifies: gh #400 gap 4 - **the probe is bounded.** A hung tmux server is
// precisely the case that must not become a startup hang, and it is the one a
// behavioural row cannot reach without wedging the suite. Asserted on the
// mechanism: a deadline and a kill, not a `wait()` that trusts the child.
#[test]
fn the_tmux_probe_cannot_wait_forever() {
    let doctor = read("crates/lca-cli/src/tui/tmux_doctor.rs");
    assert!(
        doctor.contains("public const PROBE_BUDGET") || doctor.contains("pub const PROBE_BUDGET"),
        "the budget is a named constant, so the bound is visible"
    );
    let show = doctor
        .split("fn show(option: &str, tmux: &str)")
        .nth(1)
        .and_then(|rest| rest.split("/// Run the doctor for real").next())
        .expect("show exists");
    assert!(
        show.contains("Instant::now() + PROBE_BUDGET"),
        "the deadline is set before waiting:\n{show}"
    );
    assert!(
        show.contains("child.kill()"),
        "and a probe past it is killed rather than awaited:\n{show}"
    );
    assert!(
        !show.contains(".output()"),
        "no unbounded `Command::output()` - that is the shape this guards against"
    );
}

// Verifies: gh #400 gap 4 - **`extended-keys-format` is not reported as a
// defect.** This is the guard against copying pi's advice: pi requires `csi-u`,
// and LCA's parser reads both tmux encodings, so warning that `xterm` is broken
// would be a false claim in a notice whose whole value is being right once.
//
// Asserted two ways: the decision table has no format-only failure, and the
// parser really does accept both of tmux's documented encodings.
#[test]
fn an_xterm_format_is_not_a_defect_for_lca() {
    use lca_cli::tui::tmux_doctor::{Doctor, decide};
    assert_eq!(
        decide(Some("on"), Some("xterm")),
        Doctor::Silent,
        "extended-keys on with the xterm format is healthy here"
    );
    assert_eq!(
        decide(Some("always"), Some("xterm")),
        Doctor::Silent,
        "and `always` is healthy too"
    );

    // The reason it is not a defect: both documented encodings parse. These are
    // tmux's own examples from `man tmux`'s `extended-keys-format` entry.
    assert!(
        lca_tui::engine::keys::matches_key("\x1b[27;6;65~", "ctrl+shift+a"),
        "the xterm encoding of C-S-a is understood"
    );
    assert!(
        lca_tui::engine::keys::matches_key("\x1b[65;6u", "ctrl+shift+a"),
        "the csi-u encoding of C-S-a is understood"
    );
}

// Verifies: gh #400 gap 4 - **outside tmux the doctor costs nothing.** Most users
// are not in tmux; a subprocess spawned and discarded on every launch would be a
// cost paid by the many for a notice meant for the few. The gate is asserted
// before anything else in the probe.
#[test]
fn outside_tmux_the_probe_returns_before_any_subprocess() {
    let doctor = read("crates/lca-cli/src/tui/tmux_doctor.rs");
    let examine = doctor
        .split("pub fn examine_with(")
        .nth(1)
        .and_then(|rest| rest.split("/// The launch's doctor cell").next())
        .expect("examine_with exists");
    let gate_at = examine.find("env(\"TMUX\")");
    let spawn_at = examine.find("show(EXTENDED_KEYS");
    assert!(
        matches!((gate_at, spawn_at), (Some(gate), Some(spawn)) if gate < spawn),
        "the TMUX gate precedes any probe:\n{examine}"
    );
    assert!(
        examine.contains("Doctor::Silent"),
        "and a missing TMUX returns silence, not an error"
    );
}
