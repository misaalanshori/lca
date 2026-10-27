//! `ProcessTerminal` in-module rows, split from `terminal.rs` under the
//! workspace's 1,200-line ceiling (gate 11). Behaviour unchanged.

use super::*;

#[test]
fn restart_clears_the_shutdown_flag() {
    // The external editor stops and restarts the terminal; without a
    // reset the new reader thread sees the old shutdown flag and dies.
    let mut term = ProcessTerminal::new();
    term.start(Box::new(|_| {}), Box::new(|| {}));
    term.stop();
    assert!(term.shared.shutdown.load(Ordering::SeqCst));
    term.start(Box::new(|_| {}), Box::new(|| {}));
    assert!(!term.shared.shutdown.load(Ordering::SeqCst));
    term.stop();
}

// ---- gh #369: the OSC 9;4 progress keepalive ----

/// A recorder that stands in for stdout, so the keepalive can be driven
/// without a real terminal and asserted byte for byte.
struct Recorder(Arc<Mutex<Vec<String>>>);
impl Recorder {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
    fn writes(&self) -> Vec<String> {
        self.0.lock().expect("recorder").clone()
    }
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("recorder"))
    }
}

impl ProgressGate for Recorder {
    fn write_progress_active(&self) {
        self.0
            .lock()
            .expect("recorder")
            .push(PROGRESS_ACTIVE.to_string());
    }
    fn write_progress_clear(&self) {
        self.0
            .lock()
            .expect("recorder")
            .push(PROGRESS_CLEAR.to_string());
    }
}

// Verifies: gh #369 - the defect. A turn longer than a terminal's
// 10-15 s watchdog used to lose its taskbar spinner, because
// `set_progress(true)` wrote `OSC 9;4;3` exactly once. The keepalive
// re-emits it once per `PROGRESS_KEEPALIVE_MS` for as long as the
// indicator is active, and the clock is injectable so this asserts the
// cadence without sleeping.
#[test]
fn the_keepalive_re_emits_while_progress_is_active() {
    let gate = Recorder::new();
    let mut keepalive = ProgressKeepalive::new();
    let mut now = Instant::now();
    keepalive.set_progress(&gate, true, now);
    assert_eq!(gate.take(), vec![PROGRESS_ACTIVE.to_string()], "immediate");

    // Half a window: nothing yet (the emulator's watchdog is far longer,
    // so a redundant write here would be noise).
    now += PROGRESS_KEEPALIVE_MS / 2;
    keepalive.tick(&gate, now);
    assert!(gate.take().is_empty(), "no early re-emit");

    // A full window: exactly one re-emit.
    now += PROGRESS_KEEPALIVE_MS / 2;
    keepalive.tick(&gate, now);
    assert_eq!(gate.take(), vec![PROGRESS_ACTIVE.to_string()]);

    // A long turn keeps refreshing, one per window and no more.
    for _ in 0..3 {
        now += PROGRESS_KEEPALIVE_MS;
        keepalive.tick(&gate, now);
    }
    assert_eq!(
        gate.take(),
        vec![PROGRESS_ACTIVE.to_string(); 3],
        "one re-emit per window, never a burst"
    );
}

// Verifies: gh #369 - the house rule that an idle interface never
// repaints. Once the indicator is off the keepalive must go silent, or
// a finished turn leaves the taskbar spinner running forever.
#[test]
fn the_keepalive_is_silent_once_progress_stops() {
    let gate = Recorder::new();
    let mut keepalive = ProgressKeepalive::new();
    let mut now = Instant::now();
    keepalive.set_progress(&gate, true, now);
    gate.take();
    keepalive.set_progress(&gate, false, now);
    assert_eq!(
        gate.take(),
        vec![PROGRESS_CLEAR.to_string()],
        "clearing writes once"
    );
    for _ in 0..5 {
        now += PROGRESS_KEEPALIVE_MS;
        keepalive.tick(&gate, now);
    }
    assert!(
        gate.writes().is_empty(),
        "an idle interface writes nothing: {:?}",
        gate.writes()
    );
}

// Verifies: gh #369 - `set_progress(true)` twice (a new turn while one
// is already running) does not restart the clock, so the cadence stays
// steady instead of drifting toward a burst.
#[test]
fn re_arming_keeps_the_cadence() {
    let gate = Recorder::new();
    let mut keepalive = ProgressKeepalive::new();
    let mut now = Instant::now();
    keepalive.set_progress(&gate, true, now);
    gate.take();
    now += PROGRESS_KEEPALIVE_MS / 2;
    keepalive.set_progress(&gate, true, now);
    assert!(gate.take().is_empty(), "a re-arm is not a re-emit");
    now += PROGRESS_KEEPALIVE_MS / 2;
    keepalive.tick(&gate, now);
    assert_eq!(
        gate.take(),
        vec![PROGRESS_ACTIVE.to_string()],
        "the original window still fires"
    );
}

// Verifies: gh #369 - the remembered badge survives a suspend. The
// `$EDITOR` action `stop`s the terminal (which writes `state=clear`) and
// then `start`s it; without the stored state the tab bar stays blank for
// the rest of the session, which is the issue's gap 4.
#[test]
fn a_suspended_terminal_restores_the_last_badge() {
    let mut term = ProcessTerminal::new();
    // No tty in a test process: `start` skips raw mode but still writes.
    term.set_program_status(ProgramState::Working);
    assert_eq!(term.program_status, Some(ProgramState::Working));
    // `stop` clears, and the remembered state must survive that clear.
    term.set_program_status(ProgramState::Clear);
    assert_eq!(
        term.program_status,
        Some(ProgramState::Working),
        "the last real badge is remembered across the clear"
    );
}

// Verifies: gh #369 - the keepalive's two byte sequences are the ones
// the terminal emulators document, so the emitters cannot drift from
// the protocol. (Guards against a future refactor respelling them.)
#[test]
fn the_progress_bytes_are_the_documented_sequences() {
    assert_eq!(PROGRESS_ACTIVE, "\x1b]9;4;3\x07");
    assert_eq!(PROGRESS_CLEAR, "\x1b]9;4;0\x07");
    assert_eq!(PROGRESS_KEEPALIVE_MS, Duration::from_millis(1000));
}

// Verifies: gh #369 - `tick` before any `set_progress(true)` is a
// no-op, so a loop that ticks unconditionally stays quiet at startup.
#[test]
fn ticking_before_arming_writes_nothing() {
    let gate = Recorder::new();
    let mut keepalive = ProgressKeepalive::new();
    let now = Instant::now();
    for _ in 0..5 {
        keepalive.tick(&gate, now + PROGRESS_KEEPALIVE_MS * 5);
    }
    assert!(gate.writes().is_empty(), "never armed, never writes");
}

#[test]
fn negotiation_replies_parse() {
    assert_eq!(
        parse_negotiation("\x1b[?7u"),
        Some(Negotiation::KittyFlags(7))
    );
    assert_eq!(
        parse_negotiation("\x1b[?0u"),
        Some(Negotiation::KittyFlags(0))
    );
    assert_eq!(
        parse_negotiation("\x1b[?62;1;2c"),
        Some(Negotiation::DeviceAttributes)
    );
    assert_eq!(parse_negotiation("\x1b[A"), None);
}

#[test]
fn fake_terminal_routes_input_and_output() {
    let mut term = FakeTerminal::new(80, 24);
    let got = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = got.clone();
    term.start(
        Box::new(move |s| sink.lock().unwrap().push(s)),
        Box::new(|| {}),
    );
    term.feed("\x1b[Ax");
    term.write("hello");
    assert_eq!(
        *got.lock().unwrap(),
        vec!["\x1b[A".to_string(), "x".to_string()]
    );
    assert_eq!(term.take_output(), "hello");
}

// Verifies: gh #213 - OSC 7501 traffic never reaches input, and
// status reports encode to exact bytes on the fake terminal.
#[test]
fn fake_terminal_swallows_program_status_traffic() {
    let mut term = FakeTerminal::new(80, 24);
    let got = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = got.clone();
    term.start(
        Box::new(move |s| sink.lock().unwrap().push(s)),
        Box::new(|| {}),
    );
    term.feed("\x1b]7501;?\x1b\\x");
    assert_eq!(*got.lock().unwrap(), vec!["x".to_string()]);
    term.set_program_status(ProgramState::Working);
    term.set_program_status(ProgramState::Clear);
    assert_eq!(
        term.take_output(),
        "\x1b]7501;state=working:app=lca\x1b\\\x1b]7501;state=clear\x1b\\"
    );
}

#[test]
#[allow(unsafe_code)] // SAFETY: single-threaded test process; restored immediately
fn escape_timeout_is_env_overridable() {
    unsafe { std::env::set_var("PI_TUI_ESC_TIMEOUT", "42") };
    assert_eq!(resolve_escape_timeout_ms(), 42);
    unsafe { std::env::remove_var("PI_TUI_ESC_TIMEOUT") };
}

// Verifies: R8(a) - pi's adaptive lone-ESC window. The SSH branch is
// exercised through the extracted decision, because a test that mutates
// `SSH_CONNECTION` would race every other env-sensitive test in the
// workspace.
#[test]
fn the_escape_window_adapts_to_the_transport() {
    assert_eq!(
        escape_timeout_for(None, false),
        DEFAULT_ESCAPE_TIMEOUT_MS,
        "localhost keeps pi's 10 ms window"
    );
    assert_eq!(
        escape_timeout_for(None, true),
        DEFAULT_SSH_ESCAPE_TIMEOUT_MS,
        "SSH raises it to 100 ms: latency lets ESC+char straddle packets"
    );
    assert_eq!(
        escape_timeout_for(Some(42), true),
        42,
        "the env override wins over the transport"
    );
    assert_eq!(
        escape_timeout_for(Some(0), true),
        DEFAULT_SSH_ESCAPE_TIMEOUT_MS,
        "a zero override is ignored, not adopted"
    );
}

// Verifies: R8(c) - the never-swap-a-layer rule. Exactly one key
// disambiguation layer is ever active: DA starts `modifyOtherKeys`, a
// later Kitty answer retires it, and a repeated reply changes nothing.
#[test]
fn the_negotiation_keeps_exactly_one_key_layer_active() {
    let term = ProcessTerminal::new();
    let shared = &term.shared;
    let mut out: Vec<String> = Vec::new();

    // DA arrives first: the terminal does not speak Kitty, so the
    // fallback layer starts. Sentinel, not a timeout.
    assert!(
        apply_negotiation(shared, "\x1b[?62;1;2c", &mut |s| out.push(s.to_string())),
        "DA is consumed"
    );
    assert!(shared.modify_other_keys.load(Ordering::SeqCst));
    assert!(!shared.kitty.load(Ordering::SeqCst));
    assert_eq!(out, vec!["\x1b[>4;2m".to_string()]);

    // A repeated DA adds nothing (the layer is already the active one).
    assert!(apply_negotiation(shared, "\x1b[?62;1;2c", &mut |s| out.push(s.to_string())));
    assert_eq!(out.len(), 1, "no second enable is emitted");

    // A Kitty answer retires modifyOtherKeys and starts Kitty: never both.
    assert!(apply_negotiation(shared, "\x1b[?7u", &mut |s| out.push(s.to_string())));
    assert!(shared.kitty.load(Ordering::SeqCst));
    assert!(
        !shared.modify_other_keys.load(Ordering::SeqCst),
        "the Kitty layer retired the fallback"
    );
    assert_eq!(out.last().map(String::as_str), Some("\x1b[>4;0m"));
    assert!(
        crate::engine::keys::is_kitty_protocol_active(),
        "the parser's Kitty state follows"
    );

    // Kitty arrives first elsewhere (no prior DA): it starts directly
    // and never enables the fallback.
    let term = ProcessTerminal::new();
    let shared = &term.shared;
    let mut out: Vec<String> = Vec::new();
    assert!(apply_negotiation(shared, "\x1b[?1u", &mut |s| out.push(s.to_string())));
    assert!(shared.kitty.load(Ordering::SeqCst));
    assert!(!shared.modify_other_keys.load(Ordering::SeqCst));
    assert!(
        out.is_empty(),
        "the enable sequence is write-free when it is already off"
    );
    // Leave the process-wide parser flag as the test found it.
    set_kitty_protocol_active(false);
}

/// An environment reader over a fixed map: `[(key, value), ...]`.
/// `+ use<>` keeps the argument's lifetime out of the return type: the
/// closure owns its copy of the map (edition-2024 RPIT would otherwise
/// capture it and outlive the caller's temporary array).
fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |key| map.get(key).cloned()
}

// Verifies: M2 - pi's capability ladder (terminal-image.ts), the probe
// included, decides between OSC 8 and the `text (url)` fallback.
#[test]
fn the_hyperlink_ladder_matches_pis() {
    // Unknown terminal: off, so a URL never vanishes from the screen.
    let term = env_of(&[("TERM", "xterm-256color")]);
    assert!(!detect_hyperlinks_with(|| false, term));

    // screen never forwards OSC 8.
    let term = env_of(&[("TERM", "screen-256color")]);
    assert!(!detect_hyperlinks_with(
        || panic!("no tmux under screen"),
        term
    ));

    // Known-capable terminals answer on their own environment marker.
    for key in [
        "KITTY_WINDOW_ID",
        "GHOSTTY_RESOURCES_DIR",
        "WEZTERM_PANE",
        "WARP_SESSION_ID",
        "ITERM_SESSION_ID",
        "WT_SESSION",
    ] {
        let term = env_of(&[("TERM", "xterm-256color"), (key, "1")]);
        assert!(detect_hyperlinks_with(|| false, term), "{key} is capable");
    }

    let term = env_of(&[("TERM", "xterm-256color"), ("TERM_PROGRAM", "alacritty")]);
    assert!(detect_hyperlinks_with(|| false, term));

    let term = env_of(&[
        ("TERM", "xterm-256color"),
        ("TERMINAL_EMULATOR", "jetbrains-jediterm"),
    ]);
    assert!(
        !detect_hyperlinks_with(|| false, term),
        "JetBrains' IDE terminal is off"
    );
}

// Verifies: M2 - under tmux the client's own `client_termfeatures`
// decides (pi's probeTmuxHyperlinks), not a blanket no.
#[test]
fn under_tmux_the_client_probe_decides() {
    let tmux = env_of(&[
        ("TERM", "xterm-256color"),
        ("TMUX", "/tmp/tmux-1000/default,1,0"),
    ]);
    assert!(
        detect_hyperlinks_with(|| true, tmux),
        "a forwarding client turns it on"
    );
    let tmux = env_of(&[
        ("TERM", "xterm-256color"),
        ("TMUX", "/tmp/tmux-1000/default,1,0"),
    ]);
    assert!(
        !detect_hyperlinks_with(|| false, tmux),
        "a non-forwarding client keeps it off"
    );
}

// Verifies: M2 - LCA_HYPERLINKS mirrors PI_HYPERLINKS: the override
// wins over detection (the receipt path under tmux).
#[test]
fn the_hyperlinks_override_wins_over_detection() {
    let off = env_of(&[
        ("TERM", "xterm-256color"),
        ("LCA_HYPERLINKS", "0"),
        ("KITTY_WINDOW_ID", "1"),
    ]);
    assert!(
        !detect_hyperlinks_with(|| true, off),
        "0 forces the fallback"
    );
    let on = env_of(&[("TERM", "xterm-256color"), ("LCA_HYPERLINKS", "1")]);
    assert!(detect_hyperlinks_with(|| false, on), "1 forces OSC 8");
    let junk = env_of(&[("TERM", "xterm-256color"), ("LCA_HYPERLINKS", "yes-please")]);
    assert!(
        !detect_hyperlinks_with(|| false, junk),
        "only 1|0 count as an override"
    );
}

// Verifies: gh #33 - the drop leaves the alternate screen only while it
// is still active. A second `?1049l` restores the cursor the switch
// saved (the top-left) and undoes the exit park, which is what put the
// shell prompt back on the transcript in fullscreen mode.
#[test]
fn the_drop_leaves_the_alt_screen_only_while_it_is_active() {
    let active = teardown_sequence(true);
    assert!(active.contains("\x1b[?1049l"), "{active:?}");
    assert!(active.ends_with("\x1b[?7h\x1b[?25h"), "{active:?}");

    let gone = teardown_sequence(false);
    assert!(
        !gone.contains("1049"),
        "no screen switch once the teardown has run: {gone:?}"
    );
    assert!(gone.ends_with("\x1b[?7h\x1b[?25h"), "{gone:?}");
    for seq in ["\x1b[?1000l", "\x1b[?1002l", "\x1b[?1003l", "\x1b[?1006l"] {
        assert!(gone.contains(seq), "mouse tracking is always off: {gone:?}");
    }
}

// Verifies: gh #374 gap 2 - the crash/panic teardown also stops focus reporting.
//
// The renderer's leave writes `?1004l` on an orderly exit; a **drop** is the
// path where the renderer never ran, so the reporting it turned on would survive
// into a shell prompt that reads `ESC [ O` as typing. Asserted separately from
// the mouse modes above because it is the one mode the alt screen requests that
// a shell has no business receiving.
#[test]
fn the_drop_also_stops_focus_reporting() {
    for in_alt in [true, false] {
        let out = teardown_sequence(in_alt);
        assert!(
            out.contains("\x1b[?1004l"),
            "focus reporting is turned off whatever the screen: {out:?}"
        );
        assert!(
            !out.contains("\x1b[?1004h"),
            "and the teardown never turns it on: {out:?}"
        );
    }
}

// Verifies: gh #33 - the flag the drop reads is folded from the
// renderers' own sequences, so it tracks the screen rather than a
// separate bookkeeping call.
#[test]
fn the_screen_switch_sequences_fold_into_the_drop_flag() {
    let mut term = ProcessTerminal::new();
    assert!(
        !term.in_alt_screen,
        "a fresh terminal is on the main screen"
    );

    term.write("\x1b[?1049h\x1b[?7l\x1b[2J\x1b[H\x1b[?25l");
    assert!(term.in_alt_screen, "enter switches the flag on");

    term.write("\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1049l\r\n\x1b[?7h\x1b[?25h");
    assert!(
        !term.in_alt_screen,
        "the renderer's leave switches it off, so the drop has nothing to undo"
    );
}

// ---------------------------------------------------------------------------
// gh #399: whether half-block glyph art renders (Apple_Terminal's rule)
// ---------------------------------------------------------------------------

#[test]
fn apple_terminal_is_the_platform_and_the_program_name_together() {
    // pi's rule is a *pair* (`terminal.ts:44`). Each half alone is the guess
    // this row exists to forbid: a Linux terminal can export Apple's name, and
    // a Mac can be running something else entirely.
    let mac = env_of(&[("TERM_PROGRAM", "Apple_Terminal")]);
    assert!(
        detect_apple_terminal(true, mac),
        "macOS + Apple_Terminal is the terminal pi measured"
    );

    let linux_claiming_the_name = env_of(&[("TERM_PROGRAM", "Apple_Terminal")]);
    assert!(
        !detect_apple_terminal(false, linux_claiming_the_name),
        "the name alone does not make it Apple's Terminal - half blocks render fine elsewhere"
    );

    let mac_elsewhere = env_of(&[("TERM_PROGRAM", "iTerm.app")]);
    assert!(
        !detect_apple_terminal(true, mac_elsewhere),
        "iTerm handles half blocks; only Apple_Terminal has the gap bug"
    );

    let mac_unset = env_of(&[]);
    assert!(
        !detect_apple_terminal(true, mac_unset),
        "an unset TERM_PROGRAM is unknown, not Apple's Terminal"
    );
}

#[test]
fn the_half_block_answer_can_be_forced_for_a_receipt() {
    // The fallback has to be demonstrable on a machine that is not a Mac, the
    // same way `LCA_HYPERLINKS` lets a receipt capture OSC 8 under tmux.
    let forced_off = env_of(&[("LCA_HALF_BLOCKS", "0")]);
    assert!(
        detect_apple_terminal(false, forced_off),
        "0 forces the fallback on any platform"
    );

    let forced_on = env_of(&[("LCA_HALF_BLOCKS", "1"), ("TERM_PROGRAM", "Apple_Terminal")]);
    assert!(
        !detect_apple_terminal(true, forced_on),
        "1 keeps half blocks even where the rule would fall back"
    );

    // Anything other than `0`/`1` is not an override, like `LCA_HYPERLINKS`.
    let other = env_of(&[
        ("LCA_HALF_BLOCKS", "yes"),
        ("TERM_PROGRAM", "Apple_Terminal"),
    ]);
    assert!(
        detect_apple_terminal(true, other),
        "a value that is not 0 or 1 does not count as an override"
    );
}

// Verifies: gh #379 gap 1 - the DA1 answer must reach the interface **as well as**
// being consumed by the key-layer negotiation. It is the sentinel that closes the
// startup colour query, and the negotiation layer swallowing it entirely made
// `close_theme_query` unreachable on a real terminal: the query window never shut,
// so every stray `OSC 11` printed later by a child process was applied forever.
//
// Found by driving a real pane: an injected `ESC [?1;2c` never appeared in the
// interface's input at all, while a following keystroke did.
#[test]
fn a_device_attributes_answer_reaches_the_interface_too() {
    use std::sync::mpsc;

    let term = ProcessTerminal::new();
    let shared = &term.shared;
    let (tx, rx) = mpsc::channel::<String>();
    *shared.input.lock().unwrap_or_else(|p| p.into_inner()) = Box::new(move |seq: String| {
        let _ = tx.send(seq);
    });

    let mut out: Vec<String> = Vec::new();
    assert!(
        dispatch(shared, StdinEvent::Data("\x1b[?1;2c".into()), &mut |s| out
            .push(s.to_string())),
        "the answer is still consumed by the negotiation layer"
    );
    assert_eq!(
        rx.try_recv().ok().as_deref(),
        Some("\x1b[?1;2c"),
        "and forwarded, so the theme's query can close"
    );

    // A Kitty answer is **not** the sentinel and must not be forwarded: its only
    // consumer is the negotiation layer, and forwarding it would type `ESC[?7u`.
    assert!(dispatch(
        shared,
        StdinEvent::Data("\x1b[?7u".into()),
        &mut |s| out.push(s.to_string())
    ));
    assert!(
        rx.try_recv().is_err(),
        "a Kitty flags answer is negotiation only, not input"
    );

    // Ordinary keys still reach the interface, unchanged.
    assert!(!dispatch(shared, StdinEvent::Data("a".into()), &mut |s| {
        out.push(s.to_string())
    }));
    assert_eq!(rx.try_recv().ok().as_deref(), Some("a"));
}
