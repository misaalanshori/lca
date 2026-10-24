//! Readiness primitives for the real-terminal harness (gh #447's family).
//!
//! Five unrelated `e2e_*` rows kept going red on macOS only, none of them
//! product defects. They shared a cause, and it was never the product: a row
//! asked the pane one question and assumed the answer was complete.
//!
//! - **Two-frame capture.** `wait_for(head)` returns the frame the head was
//!   painted in, then the row asserts a *second* glyph that the same paint had
//!   not put up yet (`e2e_picker_scroll`'s `╭`, `e2e_tree_navigator`'s pair of
//!   connectors, `e2e_color`'s separator still reading `Working`). The row was
//!   reading two different moments as one.
//! - **Sent-before-attached.** `[session in` is painted by the first render; it
//!   does not mean the input thread is reading keys yet, so on a loaded runner
//!   a scenario's keystrokes land in a pane that has not attached its reader
//!   and are simply gone (`e2e_slash_output`'s `/hotkeys`, `e2e_model_consent`
//!   failing in 0.82 s). No amount of polling afterwards can find output for a
//!   command that was never asked for.
//!
//! The three primitives below close those two classes *once*, so the next row
//! does not have to invent its own sleep:
//!
//! 1. [`Tmux::wait_until_painted`] — settle on a frame that has stopped
//!    changing, then assert against that frame.
//! 2. [`Tmux::wait_for_input_ready`] — make the reader's attachment a fact
//!    before the scenario is typed at it.
//! 3. [`retry_once`] — the house pattern from `conformance_diff.rs`, promoted
//!    so no adopter re-writes it.
//!
//! **None of them changes what a row asserts.** They change *when* the row
//! reads. A row that still fails on a settled frame is a real failure and keeps
//! failing — that is the entire difference between a fix and a cover-up, and it
//! is why every primitive has a negative row in `harness_contract.rs`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::Tmux;

/// How often a settle loop re-reads the pane. The same 150 ms as `wait_for`'s
/// poll, so a settle costs half a second at most in the common case and the two
/// primitives do not disagree about what "soon" means.
const POLL: Duration = Duration::from_millis(150);

/// Captures taken by settle loops, and how many settles ran, for the report and
/// for a row that wants to show the primitive actually polls more than once.
static SETTLE_CAPTURES: AtomicU64 = AtomicU64::new(0);
static SETTLES: AtomicU64 = AtomicU64::new(0);

/// `(captures taken, settles performed)` since the process started.
///
/// Exposed so a receipt can quote what settling actually cost instead of
/// claiming "it settled" and leaving the next person to guess whether the
/// primitive did anything at all.
pub fn settle_stats() -> (u64, u64) {
    (
        SETTLE_CAPTURES.load(Ordering::Relaxed),
        SETTLES.load(Ordering::Relaxed),
    )
}

/// Run `attempt`; if it missed its precondition, run it **once** more.
///
/// The house pattern from `crates/lca-ext-host/tests/conformance_diff.rs`,
/// promoted so every adopter does not re-write it (gh #447). Lazy by
/// construction: a `(attempt(), attempt())` tuple would evaluate both eagerly
/// and run the whole journey twice even when the first one is fine.
///
/// `None` means "the scenario's precondition was not met" — never "the
/// assertion failed". An assertion inside `attempt` still panics, and a panic
/// is not retried: hiding a real failure behind a second run is the worst of
/// both worlds.
pub fn retry_once<T>(mut attempt: impl FnMut() -> Option<T>) -> Option<T> {
    match attempt() {
        Some(value) => Some(value),
        None => attempt(),
    }
}

impl Tmux {
    /// Capture until two consecutive frames are byte-identical, and return
    /// that settled frame.
    ///
    /// This is the primitive for a row that reads more than one thing out of a
    /// frame: wait for the screen to stop changing, then read it once. Prefer
    /// [`Self::wait_settled_for`] when there is a needle to anchor the wait on —
    /// settling alone will happily settle on a screen that has not painted the
    /// row's content yet.
    ///
    /// # What this is not
    ///
    /// Two identical frames is a **heuristic**, not a guarantee that the app has
    /// finished. A pane that pauses for one poll interval mid-animation looks
    /// settled, and so does a screen that repaints on a timer slower than
    /// [`POLL`] (150 ms) while the middle frame happens to be stable. What would
    /// catch that, if it ever bites: a longer quiet window (require N settled
    /// frames, or a minimum quiet period rather than two samples), or asserting
    /// on a state marker the app emits rather than on the absence of motion.
    ///
    /// A **blank** pane is not settled, it is a terminal that did not come up,
    /// so this reuses `wait_for`'s healing: the same named session is re-spawned
    /// once and re-polled, which is a harness condition and never a product
    /// one.
    pub fn wait_until_painted(&self, timeout: Duration) -> String {
        self.settle(None, timeout, false)
    }

    /// [`Self::wait_until_painted`] over the pane with its SGR intact
    /// (`capture-pane -e`): settle and read in the *same* capture kind, so a
    /// colour receipt cannot be read from a frame a millisecond older than the
    /// one it settled on.
    pub fn wait_until_painted_e(&self, timeout: Duration) -> String {
        self.settle(None, timeout, true)
    }

    /// Wait until a frame that contains `needle` has also stopped changing, and
    /// return that frame.
    ///
    /// This is the primitive the two-frame rows adopt: it is `wait_for` with the
    /// second question answered — the frame the needle appeared in is not the
    /// frame the row should assert on.
    pub fn wait_settled_for(&self, needle: &str, timeout: Duration) -> String {
        self.settle(Some(needle), timeout, false)
    }

    /// [`Self::wait_settled_for`] over the SGR pane (colour receipts).
    pub fn wait_settled_for_e(&self, needle: &str, timeout: Duration) -> String {
        self.settle(Some(needle), timeout, true)
    }

    /// The settle loop behind every `wait_until_painted*`/`wait_settled_for*`.
    fn settle(&self, needle: Option<&str>, timeout: Duration, sgr: bool) -> String {
        let budget = timeout * Self::load_factor();
        let deadline = Instant::now() + budget;
        let read = || {
            SETTLE_CAPTURES.fetch_add(1, Ordering::Relaxed);
            if sgr {
                self.capture_e()
            } else {
                self.capture()
            }
        };
        let mut previous = read();
        loop {
            std::thread::sleep(POLL);
            let current = read();
            let painted = !current.trim().is_empty();
            let has_needle = needle.is_none_or(|needle| current.contains(needle));
            if painted && has_needle && current == previous {
                let settles = SETTLES.fetch_add(1, Ordering::Relaxed) + 1;
                // `LCA_SETTLE_STATS=1` makes the cost visible in a test run: how
                // many captures a row spent settling is the receipt that the
                // primitive is doing the work a `sleep` used to fake. Off by
                // default, because a passing suite should stay quiet.
                if std::env::var_os("LCA_SETTLE_STATS").is_some() {
                    eprintln!(
                        "settle: session `{}` settled after {} capture(s)                          ({} settle(s) in this binary)",
                        self.name,
                        SETTLE_CAPTURES.load(Ordering::Relaxed),
                        settles
                    );
                }
                return current;
            }
            if Instant::now() >= deadline {
                // Exhausting the budget is a failure either way, but *which*
                // failure matters to a reader: a blank pane is the terminal not
                // coming up (a harness condition, healed as in `wait_for`), and
                // a painted pane is a real product failure or a screen that
                // never stops moving.
                if !painted {
                    let what = needle.unwrap_or("a painted frame");
                    return match self.respawn_blank(what, budget) {
                        // The healed pane gets its own settle attempt, so the
                        // caller still reads a settled frame rather than a
                        // fresh one.
                        Some(pane) => pane,
                        None => panic!(
                            "`{what}`: the settled frame never painted, and the re-spawn \
                             did not either:\n{}",
                            self.capture()
                        ),
                    };
                }
                let wanted = match needle {
                    Some(needle) => format!("`{needle}`"),
                    None => "a settled frame".to_string(),
                };
                panic!(
                    "never saw {wanted} stop changing within {budget:?} \
                     (budget {timeout:?} x{} load); last two frames:\n\
                     --- previous ---\n{previous}\n--- current ---\n{current}",
                    Self::load_factor()
                );
            }
            previous = current;
        }
    }

    /// Wait until the pane's painted content actually fits `cols`, i.e. until the
    /// application has re-laid-out for the new width (gh #489).
    ///
    /// # Why this is not `wait_until_painted`
    ///
    /// A settle answers "has the screen stopped changing", and a pane that has
    /// not received its SIGWINCH is perfectly stable at the *old* width - it
    /// settles immediately and the row then asserts against a layout the terminal
    /// has already wrapped. This waits for a **property of the layout** instead:
    /// no painted line is wider than the window the pane now has.
    ///
    /// A wider-than-window line is the signature: the app drew for the spawn
    /// geometry and tmux wrapped the result, which is exactly the captured
    /// `e2e_tree_layout` failure frame (a 140-wide box inside a 60-column pane).
    ///
    /// # What this does not prove
    ///
    /// It says the app's **width** is current, not that every region has
    /// repainted - a frame can be re-flowed and still be drawing older content.
    /// Rows still wait for their own needles. The reverse case is not checked
    /// either: a *narrower* layout than the window is indistinguishable from a
    /// deliberately narrow element, so this only fails a layout that is too
    /// **wide**, which is the direction an unapplied resize produces.
    pub fn wait_for_layout_width(&self, cols: u32) -> String {
        self.wait_for_layout_width_within(cols, Duration::from_secs(20))
    }

    /// [`Self::wait_for_layout_width`] with an explicit budget, so a contract row
    /// can prove the *failure* direction without waiting out the product budget.
    pub fn wait_for_layout_width_within(&self, cols: u32, timeout: Duration) -> String {
        let limit = cols as usize;
        let budget = timeout * Self::load_factor();
        let deadline = Instant::now() + budget;
        let mut last = self.capture();
        loop {
            let widest = self.logical_width();
            if widest <= limit {
                return last;
            }
            if Instant::now() >= deadline {
                panic!(
                    "the app never re-laid-out for {cols} columns: after {budget:?} \
                     (budget {timeout:?} x{} load) the widest painted line is still \
                     {widest} cells, so the window has wrapped a layout drawn for a \
                     wider pane (gh #489). Last frame:\n{last}",
                    Self::load_factor()
                );
            }
            std::thread::sleep(POLL);
            last = self.capture();
        }
    }

    /// The widest **logical** line on the live screen, tmux's soft wraps joined.
    ///
    /// `capture-pane -p` soft-wraps an over-wide row, so every physical line then
    /// measures at most the pane width and a per-line check sees nothing wrong.
    /// `-J` undoes that join, which is the only way a layout drawn for a wider
    /// pane is visible as the defect it is (gh #489).
    pub fn logical_width(&self) -> usize {
        let out = Self::tmux(&["capture-pane", "-t", &self.name, "-p", "-J"]);
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(lca_tui::engine::text::visible_width)
            .max()
            .unwrap_or(0)
    }

    /// Make "the pane will accept a keystroke" a fact before the scenario is
    /// typed at it (gh #444, gh #446).
    ///
    /// `[session in` is painted by the first render and says nothing about the
    /// input thread; a scenario sent in that gap is lost, and lost input is
    /// invisible to every later poll. The check is an **echoed probe**: a token
    /// unique to this process is typed into the pane, waited for on screen, then
    /// erased. An echo can only come back through a live input path.
    ///
    /// # The probe never stays in the scenario
    ///
    /// The token is `zq<pid>zq` — unique enough that a false echo from unrelated
    /// content is not a real risk — and it is erased with one `Backspace` per
    /// character, then re-read to prove it is gone. A probe that cannot be
    /// erased panics rather than silently becoming the first thing the scenario
    /// says.
    ///
    /// # What this does not prove
    ///
    /// # Where this cannot be used
    ///
    /// **A masked field does not echo**, by design, so the probe can never be
    /// observed there — and the primitive would fail on a correct pane. Call it
    /// once after spawn, on the ordinary composer, before opening a `/login`
    /// secret prompt or any other no-echo surface; the attachment it proves is
    /// the same process-wide input path a later field uses.
    ///
    /// It proves the pane is *echoing*, not that `lca` is the program echoing.
    /// A shell that never started the binary echoes too. Rows still wait for
    /// their product marker first; this primitive only closes the gap between
    /// "the marker was painted" and "keys reach the reader". Over-erasing is
    /// safe because the precondition is an empty composer — which is why this
    /// belongs straight after spawn, not mid-scenario.
    pub fn wait_for_input_ready(&self, timeout: Duration) -> String {
        let budget = timeout * Self::load_factor();
        let deadline = Instant::now() + budget;
        let probe = format!("zq{}zq", std::process::id());
        // The frame the last attempt ended on: the failure message shows what
        // the pane actually had, which is what makes a deaf pane diagnosable.
        let mut last = self.capture();
        loop {
            // Literal bytes: tmux key names cannot spell some of these, and the
            // probe is meant to be unremarkable input.
            self.send_literal(&probe);
            // Short echo window per attempt: a loaded runner attaches the reader
            // late, and a lost probe is exactly what a retry is for.
            let echo_deadline = Instant::now() + Duration::from_millis(400);
            while Instant::now() < echo_deadline {
                let pane = self.capture();
                if pane.contains(&probe) {
                    // Erase before returning: the scenario must see an empty
                    // composer, not the harness's probe.
                    self.erase_probe(probe.chars().count());
                    return pane;
                }
                last = pane;
                std::thread::sleep(Duration::from_millis(40));
            }
            // The whole probe was not echoed. Part of it may have landed, so
            // clear generously before trying again (Backspace on an empty
            // composer is a no-op, which is why this is safe here).
            self.erase_probe(probe.chars().count() + 2);
            if Instant::now() >= deadline {
                panic!(
                    "the pane never echoed a probe keystroke within {budget:?} \
                     (budget {timeout:?} x{} load), so input is not attached; \
                     a scenario sent now would be lost (gh #444/#446). \
                     Last frame:\n{last}",
                    Self::load_factor()
                );
            }
            std::thread::sleep(POLL);
        }
    }

    /// `count` backspaces, then wait until the probe token is really gone.
    ///
    /// tmux's key name is `BSpace`; `Backspace` is not a key name, so
    /// `send-keys` would send the literal word — which the first version of this
    /// did, and the erase assertion is what caught it.
    fn erase_probe(&self, count: usize) {
        let keys = vec!["BSpace"; count];
        self.send(&keys);
        let deadline = Instant::now() + Duration::from_secs(2) * Self::load_factor();
        loop {
            if Instant::now() >= deadline {
                // Loud on purpose: a probe left in the composer would become
                // part of the scenario's prompt.
                panic!(
                    "the input probe could not be erased; the scenario would \
                     start with it:\n{}",
                    self.capture()
                );
            }
            std::thread::sleep(Duration::from_millis(40));
            let pane = self.capture();
            if !pane.contains(&format!("zq{}zq", std::process::id())) {
                return;
            }
        }
    }
}
