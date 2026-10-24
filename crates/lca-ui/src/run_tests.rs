//! `run` loop tests (R-series rows), split from `run.rs` for the
//! workspace's 1,200-line file ceiling. Behavior unchanged.

use super::*;
use crate::run_keybindings::keybinding_problems;
use lca_tui::engine::terminal::FakeTerminal;

// Verifies: gh #66 - the loud report names every problem class and
// stays silent when the file parsed clean.
#[test]
fn the_keybinding_report_names_every_problem() {
    assert_eq!(keybinding_problems(None, &[], &[], &[]), None);
    let report = keybinding_problems(
        Some("not valid TOML".to_string()),
        &["app.typo".to_string()],
        &[("tui.editor.cursorLeft".to_string(), "ctrl+xyz".to_string())],
        &[lca_tui::engine::keybindings::KeybindingConflict {
            key: "ctrl+x".to_string(),
            keybindings: vec!["a".to_string(), "b".to_string()],
        }],
    )
    .expect("a report");
    assert!(report.contains("not valid TOML"), "{report}");
    assert!(report.contains("app.typo"), "{report}");
    assert!(report.contains("ctrl+xyz"), "{report}");
    assert!(report.contains("ctrl+x"), "{report}");
}

// Verifies: FR-UI-21 - the runtime toggle swaps the renderer both ways.
#[test]
fn switch_screen_toggles_the_renderer() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    assert!(!screen.is_fullscreen());
    switch_screen(&mut screen, true, &mut term);
    assert!(screen.is_fullscreen());
    // Entering the alt screen wrote the enable sequence.
    assert!(term.output().contains("\x1b[?1049h"));
    switch_screen(&mut screen, false, &mut term);
    assert!(!screen.is_fullscreen());
}

fn options() -> UiOptions {
    UiOptions {
        prompt_slot: Default::default(),
        dialog_slot: Default::default(),
        pending_models: None,
        model_label: Arc::new(std::sync::Mutex::new("p/m".into())),
        context_window: Arc::new(std::sync::Mutex::new(0)),
        thinking: Arc::new(std::sync::Mutex::new(None)),
        theme: "auto".to_string(),
        theme_extra_dirs: Vec::new(),
        themes: crate::theme::THEMES.iter().map(|s| s.to_string()).collect(),
        initial_lines: Vec::new(),
        initial_header: (Vec::new(), Vec::new()),
        initial_records: Vec::new(),
        initial_tail_lines: Vec::new(),
        initial_messages: Vec::new(),
        open_resume_picker: false,
        yolo: false,
        early_input: None,
        thinking_visibility: Default::default(),
        codeblock_border: Default::default(),
        plain: true,
        invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
        slash_commands: Vec::new(),
        models: Vec::new(),
        workspace: std::path::PathBuf::from("."),
        keybinding_overrides: Default::default(),
        keybinding_error: None,
        render_regions: None,
        ui_events: None,
        update_notice: None,
        login: None,
        complete_login: None,
        pick_login: None,
        confirm_login_grant: None,
        confirm_switch: None,
        hooks: crate::state::UiHooks::default(),
        fullscreen: false,
        follow_up_mode: lca_protocol::QueueMode::OneAtATime,
    }
}

pub(crate) fn chat() -> Chat {
    Chat::new(options(), Arc::new(KeybindingsManager::new()))
}

// Verifies: gh #9 / R6 - the copy ladder writes the text through the
// host's verified native clipboard when it has one, falls back to
// OSC 52 when the screen can send it, and says which step ran; with
// neither available it says the copy did not happen instead of
// pretending.
#[test]
fn the_copy_ladder_writes_the_text_and_says_which_step_ran() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let hook: crate::state::ClipboardWriter = {
        let seen = seen.clone();
        Arc::new(move |text: &str| {
            seen.lock().unwrap().push(text.to_string());
            true
        })
    };
    let report = copy_through_ladder("hello world", Some(&hook), None);
    assert_eq!(report, super::CopyReport::Native);
    assert_eq!(report.notice(), "copied to clipboard");
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["hello world".to_string()],
        "the text reaches the clipboard verbatim"
    );

    let mut sent = Vec::new();
    let report = copy_through_ladder(
        "osc",
        None,
        Some(&mut |text: &str| sent.push(text.to_string())),
    );
    let notice = report.notice();
    assert_eq!(sent, vec!["osc"], "OSC 52 carries the same text");
    assert!(
        notice.contains("nothing was copied"),
        "the unverified notice is honest: {notice}"
    );

    let notice = copy_through_ladder("nowhere", None, None).notice();
    assert!(
        notice.starts_with("could not copy"),
        "no step ran, so no claim: {notice}"
    );
}

// Verifies: gh #9 (pi 1.0.0's `app.message.copy`) - with nothing
// selected, Ctrl+X copies the last assistant message through that
// ladder: this host has no native clipboard tool, so OSC 52 reaches
// the terminal with the message's base64, the notice says the copy
// is unverified, and the same press still arms the Ctrl+X Ctrl+E
// chord (FR-UI-15's row is unchanged).
#[test]
fn ctrl_x_copies_the_last_assistant_message_through_the_ladder() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut term);
    let mut chat = chat();
    chat.on_turn_event(lca_protocol::TurnEvent::TextDelta("copy me".into()));
    let (input_tx, _input_rx) = std::sync::mpsc::channel();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
    let outcome = handle_input(
        "\x18",
        &mut chat,
        &mut screen,
        &mut term,
        &input_tx,
        &resize_tx,
        None,
        &None,
        &mut false,
    );
    assert!(matches!(outcome, InputResult::Continue));
    let out = term.output();
    assert!(
        out.contains("\x1b]52;c;"),
        "OSC 52 reached the terminal: {out:?}"
    );
    assert!(
        out.contains("Y29weSBtZQ=="),
        "the message, base64 (`copy me`): {out:?}"
    );
    assert_eq!(
        chat.toasts.last().map(|toast| toast.text.as_str()),
        Some("copied (if your terminal blocked the clipboard, nothing was copied)"),
        "the honest unverified notice"
    );
    // The same press armed the external-editor chord.
    assert_eq!(chat.handle_key("\x05"), Action::ExternalEditor);
}

// Verifies: gh #9 (the selection half of `app.message.copy`) - a
// mouse selection still copies on release through the same ladder
// and the same honest notice the refactor kept: this row exists
// because the key path now shares that ladder with it.
#[test]
fn a_mouse_selection_still_copies_on_release_through_the_ladder() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut term);
    let mut chat = chat();
    let (input_tx, _input_rx) = std::sync::mpsc::channel();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
    // A frame with text first, so the selection has rows to read.
    screen.render(&mut term, vec!["hello world".to_string()], 80, 24);
    let mut drive = |data: &str| {
        handle_input(
            data,
            &mut chat,
            &mut screen,
            &mut term,
            &input_tx,
            &resize_tx,
            None,
            &None,
            &mut false,
        )
    };
    drive("\x1b[<0;1;1M");
    drive("\x1b[<32;6;1M");
    let outcome = drive("\x1b[<0;6;1m");
    assert!(matches!(outcome, InputResult::Continue));
    let out = term.output();
    assert!(
        out.contains("\x1b]52;c;") && out.contains("aGVsbG8="),
        "the selection (`hello`, base64) went out over OSC 52: {out:?}"
    );
    assert_eq!(
        chat.toasts.last().map(|toast| toast.text.as_str()),
        Some("copied (if your terminal blocked the clipboard, nothing was copied)"),
        "the same notice the mouse path always gave"
    );
    // The key path did not also fire: one press, one copy.
    assert!(!out.contains("Y29weSBtZQ=="), "nothing else was copied");
}

// Verifies: gh #35 - End (pi's `tui.altScreen.bottom`) returns the
// fullscreen viewport to the live bottom, and only fullscreen: in
// main-screen mode - and while a modal owns the keyboard - the key
// is not a viewport key at all, so the editor keeps it.
#[test]
fn end_returns_the_fullscreen_viewport_to_the_live_bottom() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut term);
    let mut chat = chat();
    chat.screen_mode = true;
    screen.set_scroll(12);
    let (input_tx, _input_rx) = std::sync::mpsc::channel();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
    let outcome = handle_input(
        "\x1b[F",
        &mut chat,
        &mut screen,
        &mut term,
        &input_tx,
        &resize_tx,
        None,
        &None,
        &mut false,
    );
    assert!(matches!(outcome, InputResult::Continue));
    assert_eq!(screen.scroll(), 0, "End jumped to the live bottom");

    // The gate's own half: main-screen mode and an open modal never
    // claim the key.
    chat.screen_mode = false;
    assert!(
        !chat.alt_screen_bottom("\x1b[F"),
        "main screen: End stays with the editor"
    );
    chat.screen_mode = true;
    chat.world.show_permission("rm -rf /tmp/x".into());
    assert!(
        !chat.alt_screen_bottom("\x1b[F"),
        "a modal owns the keyboard, not the viewport"
    );
}

// Verifies: gh #33 - the exit teardown (`preserve = false`) parks the
// cursor below the transcript in main-screen mode too; `Screen::leave`
// used to delegate only to `Screen::Alt`, so the main-screen path
// emitted nothing and the shell prompt landed on LCA's content.
#[test]
fn screen_leave_parks_the_cursor_in_main_screen_mode() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    screen.render(
        &mut term,
        vec!["transcript".into(), "footer".into()],
        80,
        24,
    );
    let _ = term.take_output();

    screen.leave(&mut term, false);
    let out = term.take_output();

    assert!(
        out.ends_with("\r\n\x1b[?7h\x1b[?25h"),
        "the exit write parks the cursor on a fresh line: {out:?}"
    );
}

// Verifies: gh #33's call-site audit - `preserve = true` is the
// mid-run handoff (the external editor and the fullscreen switch), not
// an exit, so it must still write nothing in main-screen mode.
#[test]
fn a_preserving_leave_in_main_screen_mode_writes_nothing() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    screen.render(&mut term, vec!["transcript".into()], 80, 24);
    let _ = term.take_output();

    screen.leave(&mut term, true);

    assert!(
        term.take_output().is_empty(),
        "a handoff leaves the terminal exactly as it was"
    );
}

// Verifies: S7 - a link click names the reason when the opener fails,
// so "cannot open" never hides a missing launcher behind a bare no.
#[test]
fn a_failed_link_open_says_why() {
    assert_eq!(link_notice("https://x", Some(Ok(()))), "opened https://x");
    assert_eq!(
        link_notice(
            "https://x",
            Some(Err("no web browser found to open link".into()))
        ),
        "cannot open https://x: no web browser found to open link"
    );
    assert_eq!(link_notice("https://x", None), "cannot open https://x");
}

// Verifies: R19 - the auto-approve countdown (FR-UI-18) fires exactly at
// its deadline, with the same decision pressing Allow would send.
#[test]
fn the_auto_approve_countdown_fires_at_the_deadline() {
    let mut chat = chat();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    chat.world.permission = Some(PermissionModal {
        action: "rm -rf /tmp/x".into(),
        respond: Some(tx),
        deadline: Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
    });
    // Not due: it keeps the modal and asks for a repaint.
    assert!(tick_permission(&mut chat));
    assert!(chat.world.permission.is_some());
    assert!(rx.try_recv().is_err(), "nothing fired yet");
    // Due: it fires `Once` and closes the modal.
    if let Some(modal) = chat.world.permission.as_mut() {
        modal.deadline = Some(std::time::Instant::now());
    }
    assert!(tick_permission(&mut chat));
    assert!(chat.world.permission.is_none());
    assert_eq!(rx.try_recv().ok(), Some(lca_permissions::Decision::Once));
    // Nothing open: nothing to do.
    assert!(!tick_permission(&mut chat));
}

// Verifies: gh #11 - a click's bytes travel the loop's input path:
// SGR press+release through the alt-screen renderer hit-tests the
// transcript and toggles the reasoning run, end to end (the same
// bytes in main-screen mode stay with the terminal: no toggle).
#[test]
fn a_click_on_a_reasoning_row_toggles_the_run_through_the_loop() {
    use lca_tui::engine::text::strip_terminal_sequences;
    let strip = |lines: &[String]| -> Vec<String> {
        lines.iter().map(|l| strip_terminal_sequences(l)).collect()
    };
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut term);
    let mut chat = chat();
    chat.screen_mode = true;
    chat.world.resize(80, 24);
    chat.transcript.push_user("quux question");
    chat.on_turn_event(lca_protocol::TurnEvent::ReasoningDelta(
        "alpha\nbeta\ngamma\ndelta\nepsilon".into(),
    ));
    chat.on_turn_event(lca_protocol::TurnEvent::TextDelta("the answer".into()));
    chat.on_turn_event(lca_protocol::TurnEvent::TurnEnded {
        status: lca_protocol::TurnStatus::Ok,
        stop_reason: lca_protocol::StopReason::Stop,
    });
    let frame = strip(&chat.viewport(80, 24, 0));
    assert!(!frame.iter().any(|l| l.contains("delta")));
    let row = frame
        .iter()
        .position(|l| l.contains("alpha"))
        .expect("a visible reasoning row") as u16;
    let (input_tx, _input_rx) = std::sync::mpsc::channel();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
    let press = format!("\x1b[<0;5;{}M", row + 1);
    let release = format!("\x1b[<0;5;{}m", row + 1);
    // Direct calls (no closure): each call's borrows end on return,
    // so the frame reads between them compile.
    let outcome = handle_input(
        &press,
        &mut chat,
        &mut screen,
        &mut term,
        &input_tx,
        &resize_tx,
        None,
        &None,
        &mut false,
    );
    assert!(matches!(outcome, InputResult::Continue));
    let outcome = handle_input(
        &release,
        &mut chat,
        &mut screen,
        &mut term,
        &input_tx,
        &resize_tx,
        None,
        &None,
        &mut false,
    );
    assert!(matches!(outcome, InputResult::Continue));
    let frame = strip(&chat.viewport(80, 24, 0));
    assert!(
        frame.iter().any(|l| l.contains("delta")),
        "the click toggled the run through the loop:\n{}",
        frame.join("\n")
    );

    // Main-screen: the same bytes never reach the transcript.
    chat.screen_mode = false;
    switch_screen(&mut screen, false, &mut term);
    // Collapse the run again first (Ctrl+T), so a toggle would show.
    chat.transcript.toggle_thinking_expanded();
    let frame = strip(&chat.viewport(80, 24, 0));
    assert!(!frame.iter().any(|l| l.contains("delta")));
    let outcome = handle_input(
        &press,
        &mut chat,
        &mut screen,
        &mut term,
        &input_tx,
        &resize_tx,
        None,
        &None,
        &mut false,
    );
    assert!(matches!(outcome, InputResult::Continue));
    let outcome = handle_input(
        &release,
        &mut chat,
        &mut screen,
        &mut term,
        &input_tx,
        &resize_tx,
        None,
        &None,
        &mut false,
    );
    assert!(matches!(outcome, InputResult::Continue));
    let frame = strip(&chat.viewport(80, 24, 0));
    assert!(
        !frame.iter().any(|l| l.contains("delta")),
        "main-screen clicks stay with the terminal:\n{}",
        frame.join("\n")
    );
}

// Verifies: gh #172 - a button click's bytes travel the loop's input
// path: SGR press+release at the button's cell reaches the extension
// as `ClickWidget` through the alt-screen renderer.
#[test]
fn a_click_on_an_extension_button_names_its_widget_through_the_loop() {
    use lca_protocol::{UiInput, Widget};
    let seen: std::sync::Arc<std::sync::Mutex<Vec<UiInput>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_in = seen.clone();
    let mut opts = options();
    opts.render_regions = Some(std::sync::Arc::new(|region: &str| {
        if region != "modal" {
            return Vec::new();
        }
        vec![(
            "click-demo".to_string(),
            lca_protocol::WidgetTree {
                nodes: vec![Widget::Button {
                    id: "fire".into(),
                    label: "FIRE".into(),
                }],
            },
        )]
    }));
    opts.ui_events = Some(std::sync::Arc::new(move |_, input: &UiInput| {
        seen_in.lock().unwrap().push(input.clone());
        None
    }));
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut term);
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.screen_mode = true;
    chat.world.modal_open = true;
    // Find the button's cell through the mapping itself, then prove
    // the loop's bytes land on it.
    let mut target = None;
    for row in 0..24u16 {
        for col in 0..80u16 {
            seen.lock().unwrap().clear();
            if matches!(
                chat.click_extension(col, row, 80, 24),
                Some(crate::chat_mouse::ExtClick::Event(
                    _,
                    UiInput::ClickWidget { .. }
                ))
            ) {
                target = Some((col, row));
            }
        }
    }
    let (col, row) = target.expect("a clickable button cell");
    seen.lock().unwrap().clear();
    let (input_tx, _input_rx) = std::sync::mpsc::channel();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
    let press = format!("\x1b[<0;{};{}M", col + 1, row + 1);
    let release = format!("\x1b[<0;{};{}m", col + 1, row + 1);
    for data in [&press, &release] {
        let outcome = handle_input(
            data,
            &mut chat,
            &mut screen,
            &mut term,
            &input_tx,
            &resize_tx,
            None,
            &None,
            &mut false,
        );
        assert!(matches!(outcome, InputResult::Continue));
    }
    assert!(
        seen.lock().unwrap().iter().any(|input| matches!(
            input,
            UiInput::ClickWidget { id } if id == "fire"
        )),
        "the click named the widget through the loop"
    );
}

// Verifies: gh #172 - wheel bytes over a panel scroll container travel
// the loop's input path and move the region's offset.
#[test]
fn wheel_over_a_panel_scrolls_through_the_loop() {
    use lca_protocol::Widget;
    let mut opts = options();
    opts.render_regions = Some(std::sync::Arc::new(|region: &str| {
        if region != "panel" {
            return Vec::new();
        }
        vec![(
            "wheel-demo".to_string(),
            lca_protocol::WidgetTree {
                nodes: vec![Widget::ScrollContainer {
                    max_height: 2,
                    children: vec![1, 2, 3],
                }],
            },
        )]
    }));
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut term);
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    chat.screen_mode = true;
    chat.world.panel_open = true;
    let (input_tx, _input_rx) = std::sync::mpsc::channel();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
    // Wheel down (button bit 1) over the panel's right columns.
    let outcome = handle_input(
        "\x1b[<65;79;2M",
        &mut chat,
        &mut screen,
        &mut term,
        &input_tx,
        &resize_tx,
        None,
        &None,
        &mut false,
    );
    assert!(matches!(outcome, InputResult::Continue));
    assert_eq!(chat.world.ext_scroll.get("panel"), Some(&1));
}

// Verifies: gh #200 receipt - Ctrl+X on the OAuth waiting screen
// copies the exact sign-in URL through the native clipboard and the
// modal confirms with the named notice.
#[test]
fn ctrl_x_copies_the_oauth_url_with_the_named_notice() {
    let mut term = FakeTerminal::new(80, 24);
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut term);
    let mut opts = options();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_hook = seen.clone();
    opts.hooks.copy_to_clipboard = Some(Arc::new(move |text: &str| {
        seen_hook.lock().unwrap().push(text.to_string());
        true
    }));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    let url = "https://accounts.example.test/auth?code=42";
    chat.apply_login_next(crate::state::LoginNext::Waiting {
        label: format!(
            "waiting for browser sign-in… (esc cancels)\n\n\x1b]8;;{url}\x07{url}\x1b]8;;\x07"
        ),
    });
    let (input_tx, _input_rx) = std::sync::mpsc::channel();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel();
    let outcome = handle_input(
        "\x18",
        &mut chat,
        &mut screen,
        &mut term,
        &input_tx,
        &resize_tx,
        None,
        &None,
        &mut false,
    );
    assert!(matches!(outcome, InputResult::Continue));
    assert_eq!(
        *seen.lock().unwrap(),
        vec![url.to_string()],
        "the exact URL reaches the clipboard"
    );
    assert_eq!(
        chat.toasts.last().map(|toast| toast.text.as_str()),
        Some("\u{2713} Copied sign-in URL to clipboard"),
        "the named confirmation"
    );
}

// Verifies: gh #173 - Shift+wheel steps between prompts (up steps back,
// down steps forward); a plain wheel or a shifted non-wheel gesture is
// not a step.
#[test]
fn shift_wheel_gestures_step_and_plain_ones_do_not() {
    use lca_tui::engine::alt_screen::SgrMouse;
    let gesture = |bits: u16| SgrMouse {
        bits,
        x: 1,
        y: 1,
        press: true,
    };
    assert_eq!(
        crate::run_selection::shift_wheel_jump(&gesture(64 + 4)),
        Some(-1)
    );
    assert_eq!(
        crate::run_selection::shift_wheel_jump(&gesture(65 + 4)),
        Some(1)
    );
    assert_eq!(crate::run_selection::shift_wheel_jump(&gesture(64)), None);
    assert_eq!(crate::run_selection::shift_wheel_jump(&gesture(65)), None);
    assert_eq!(crate::run_selection::shift_wheel_jump(&gesture(4)), None);
    assert_eq!(crate::run_selection::shift_wheel_jump(&gesture(0)), None);
}

// Verifies: gh #82 - the exit hint names the live session for
// resume-hint mode, and stays quiet otherwise.
#[test]
fn exit_hint_names_the_session_only_for_resume_hint() {
    let tuning = crate::state::DisplayTuning {
        fullscreen_exit_output: "resume-hint".to_string(),
        ..crate::state::DisplayTuning::default()
    };
    assert_eq!(
        super::exit_hint(&tuning, "abc123"),
        Some("session continues in scrollback - resume with: lca --resume abc123".to_string())
    );
    assert_eq!(super::exit_hint(&tuning, ""), None, "no id, no hint");
    let tuning = crate::state::DisplayTuning::default();
    assert_eq!(
        super::exit_hint(&tuning, "abc123"),
        None,
        "transcript exits quietly"
    );
}

// Verifies: gh #82 - the screen carries the tunables on switch.
#[test]
fn screen_switch_applies_copy_and_wheel_tuning() {
    let tuning = crate::state::DisplayTuning {
        fullscreen_copy_on_select: false,
        ..crate::state::DisplayTuning::default()
    };
    let mut screen = Screen::Main(MainScreenRenderer::new());
    switch_screen(&mut screen, true, &mut FakeTerminal::new(80, 24));
    match &screen {
        Screen::Alt(alt) => assert!(alt.copy_on_select, "on by default"),
        Screen::Main(_) => panic!("should be fullscreen"),
    }
    crate::run_selection::apply_screen_tuning(&mut screen, &tuning);
    match &screen {
        Screen::Alt(alt) => {
            assert!(!alt.copy_on_select, "config wins over the default");
        }
        Screen::Main(_) => panic!("should be fullscreen"),
    }
}

// Verifies: gh #233 - consent-delivered rows invalidate the snapshot,
// so the next `/model` re-enumerates past the grant.
#[test]
fn pending_models_delivery_invalidates_the_snapshot() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut opts = options();
    opts.hooks.models = Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        vec![("m".to_string(), "M".to_string())]
    }));
    opts.pending_models = Some(Arc::new(|| vec![("n".to_string(), "N".to_string())]));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.model_picker.is_some());
    // gh #232: the drain makes the background arrival deterministic.
    chat.drain_model_refresh();
    chat.handle_key("\x1b");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(super::poll_pending_models(&mut chat), "rows deliver");
    assert!(chat.model_picker.is_some(), "consent opens the picker");
    chat.handle_key("\x1b");
    for c in "/model".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    chat.drain_model_refresh();
    assert_eq!(calls.load(Ordering::SeqCst), 2, "delivery invalidates");
}

// Verifies: gh #213 - the tab badge follows idle, working, blocked,
// and done, emitting on change only (repeats stay silent).
#[test]
fn program_status_follows_the_lifecycle() {
    let (prompt_tx, prompt_rx) = std::sync::mpsc::sync_channel(4);
    let (_dialog_tx, dialog_rx) = std::sync::mpsc::sync_channel(4);
    let mut turns = TurnState::new(prompt_tx, prompt_rx, dialog_rx);
    let mut chat = chat();
    let mut term = FakeTerminal::new(80, 24);
    turns.sync_program_status(&chat, &mut term);
    chat.turn_running = true;
    turns.sync_program_status(&chat, &mut term);
    turns.sync_program_status(&chat, &mut term);
    chat.world.modal_open = true;
    turns.sync_program_status(&chat, &mut term);
    chat.world.modal_open = false;
    chat.turn_running = false;
    turns.last_ok = Some(true);
    turns.sync_program_status(&chat, &mut term);
    // SANCTIONED CHANGE (gh #369): this row used to compare the raw stream,
    // which now also carries the taskbar spinner's `OSC 9;4` arm/disarm (a
    // running turn arms it, the resting edge clears it). The badge contract
    // this row pins is unchanged, so it reads the 7501 channel alone - see
    // `progress_arms_for_compaction_and_shell_runs` for the 9;4 side.
    assert_eq!(
        badges(&term.take_output()),
        concat!(
            "\x1b]7501;state=idle:app=lca\x1b\\",
            "\x1b]7501;state=working:app=lca\x1b\\",
            "\x1b]7501;state=blocked:app=lca\x1b\\",
            "\x1b]7501;state=done:app=lca\x1b\\",
        )
    );
}

// Verifies: gh #401 item 1 (pi's `onEscape` -> `restoreQueuedMessagesToEditor`)
// - an aborted turn returns the queue to the editor from the LOOP's own
// abort path (`TurnState::reap`), not merely from `Chat::restore_pending`.
// This row fails the moment `chat.restore_pending()` leaves the abort
// branch: the queue then stays in `pending` and the editor stays empty.
#[test]
fn an_aborted_turn_restores_the_queue_through_the_reap_path() {
    let (prompt_tx, prompt_rx) = std::sync::mpsc::sync_channel(4);
    let (_dialog_tx, dialog_rx) = std::sync::mpsc::sync_channel(4);
    let mut turns = TurnState::new(prompt_tx, prompt_rx, dialog_rx);
    let mut chat = chat();
    chat.begin_turn(lca_protocol::steer_queue());
    chat.queue_submit("steer me".into(), lca_protocol::SubmitMode::Steer);
    chat.queue_submit("follow me".into(), lca_protocol::SubmitMode::FollowUp);
    // The user typed while the turn ran; the draft must survive the merge.
    chat.editor.set_text("half-typed");
    // A finished turn handle, as the Escape key leaves behind.
    let handle = std::thread::spawn(|| lca_protocol::TurnOutcome {
        status: lca_protocol::TurnStatus::Error,
        stop_reason: lca_protocol::StopReason::Cancelled,
        usage: lca_protocol::Usage::default(),
        error: None,
    });
    turns.active = Some(handle);
    turns.aborted = true;
    // Joining a freshly-spawned handle: give the thread a moment to land.
    for _ in 0..200 {
        if turns.active.as_ref().is_some_and(|h| h.is_finished()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(turns.reap(&mut chat), "the turn reaped");
    assert!(chat.pending.is_empty(), "the queue left the dock");
    assert_eq!(
        chat.editor.text(),
        "steer me\n\nfollow me\n\nhalf-typed",
        "queue first, draft last: nothing lost, nothing auto-sent"
    );
    assert_eq!(
        chat.last_notice().as_deref(),
        Some("Restored 2 queued messages to editor")
    );
}

// ---- gh #369: compaction and shell runs badge `working` ----

/// Only the OSC 7501 bytes from a frame's output (gh #369): the same frame
/// also drives `OSC 9;4`, so a badge assertion filters to its own channel.
/// Sequences are ST-terminated (`\x1b\\`), which is why the split keeps it.
fn badges(output: &str) -> String {
    let mut out = String::new();
    let mut rest = output;
    while let Some(start) = rest.find("\x1b]7501;") {
        let tail = &rest[start..];
        let end = tail.find("\x1b\\").map_or(tail.len(), |i| i + 2);
        out.push_str(&tail[..end]);
        rest = &tail[end..];
    }
    out
}

/// A `TurnState` with nothing in flight.
fn quiet_turns() -> TurnState {
    let (prompt_tx, prompt_rx) = std::sync::mpsc::sync_channel(4);
    let (_dialog_tx, dialog_rx) = std::sync::mpsc::sync_channel(4);
    TurnState::new(prompt_tx, prompt_rx, dialog_rx)
}

// Verifies: gh #369 - a background `/compact` is work the user is waiting
// on, so the tab badges `working`. Before this, `sync_program_status`
// passed only `chat.turn_running`, so a compaction running in another
// window left the badge on its resting state.
#[test]
fn a_background_compaction_badges_working() {
    let mut turns = quiet_turns();
    let mut chat = chat();
    let mut term = FakeTerminal::new(80, 24);
    turns.sync_program_status(&chat, &mut term);
    term.take_output();
    chat.compacting = true;
    turns.sync_program_status(&chat, &mut term);
    assert_eq!(
        badges(&term.take_output()),
        "\x1b]7501;state=working:app=lca\x1b\\",
        "compaction badges working"
    );
    // And the resting edge puts the badge back.
    chat.compacting = false;
    turns.last_ok = Some(true);
    turns.sync_program_status(&chat, &mut term);
    assert_eq!(
        badges(&term.take_output()),
        "\x1b]7501;state=done:app=lca\x1b\\",
        "and it settles to done"
    );
}

// Verifies: gh #369 - an inline `!`/`!!` shell run badges `working` for the
// same reason: the user started it and is waiting on it.
#[test]
fn a_shell_run_badges_working() {
    let mut turns = quiet_turns();
    let mut chat = chat();
    let mut term = FakeTerminal::new(80, 24);
    assert!(!chat.shell_running(), "nothing runs yet");
    // Drive the real shell state rather than a stand-in field: a `ShellRun`
    // is just its channels, so no process is spawned.
    let (_tx, rx) = std::sync::mpsc::channel();
    chat.shell = Some(crate::chat_pickers::ShellRun {
        output: rx,
        cancel: std::sync::Arc::new(|| {}),
        command: "echo hi".to_string(),
        excluded: false,
    });
    assert!(chat.shell_running());
    turns.sync_program_status(&chat, &mut term);
    assert_eq!(
        badges(&term.take_output()),
        "\x1b]7501;state=working:app=lca\x1b\\",
        "a shell run badges working"
    );
    chat.shell = None;
    turns.sync_program_status(&chat, &mut term);
    assert_eq!(
        badges(&term.take_output()),
        "\x1b]7501;state=idle:app=lca\x1b\\",
        "and it settles back to idle"
    );
}

// Verifies: gh #369 - transitions only. Repeating a frame with the same
// working state writes nothing, which is the "no per-frame spam" rule and
// what keeps an idle interface from repainting.
#[test]
fn badge_and_progress_emit_on_transition_only() {
    let mut turns = quiet_turns();
    let mut chat = chat();
    let mut term = FakeTerminal::new(80, 24);
    chat.compacting = true;
    turns.sync_program_status(&chat, &mut term);
    assert!(!term.take_output().is_empty(), "the entry frame writes");
    for _ in 0..5 {
        turns.sync_program_status(&chat, &mut term);
        assert!(
            term.take_output().is_empty(),
            "a repeated frame writes nothing"
        );
    }
}

// Verifies: gh #369 - the taskbar spinner arms for a compaction and a shell
// run too, not just a turn (acceptance 2). `FakeTerminal` records the
// `set_progress` calls, so this asserts the arm/disarm edges without a real
// terminal.
#[test]
fn progress_arms_for_compaction_and_shell_runs() {
    let mut turns = quiet_turns();
    let mut chat = chat();
    let mut term = FakeTerminal::new(80, 24);

    chat.compacting = true;
    turns.sync_program_status(&chat, &mut term);
    assert!(term.progress(), "compaction arms the spinner");

    chat.compacting = false;
    turns.last_ok = Some(true);
    turns.sync_program_status(&chat, &mut term);
    assert!(!term.progress(), "and it disarms when compaction ends");

    let (_tx, rx) = std::sync::mpsc::channel();
    chat.shell = Some(crate::chat_pickers::ShellRun {
        output: rx,
        cancel: std::sync::Arc::new(|| {}),
        command: "echo hi".to_string(),
        excluded: false,
    });
    turns.sync_program_status(&chat, &mut term);
    assert!(term.progress(), "a shell run arms the spinner");
    chat.shell = None;
    turns.sync_program_status(&chat, &mut term);
    assert!(!term.progress(), "and it disarms when the run ends");
}

// Verifies: gh #387 gap 1 **on the live path**, not just in the palette: an
// OSC 11 background reply arriving through the real `handle_input` rebuilds the
// `system` theme, and the rendered user band then carries a SGR background
// distinct from the probed one.
//
// This is the row the panes could not give me: `tmux` 3.5a has no
// `default-background` option and forwards OSC 11 to a parent terminal that does
// not exist in CI, so a real pane never receives a reply. Driving `handle_input`
// with the reply a terminal *would* send is the same code path minus the
// terminal.
#[test]
fn an_osc11_reply_rebuilds_the_system_panels() {
    use lca_tui::engine::colors::RgbColor;
    let mut term = FakeTerminal::new(100, 30);
    // The product default: a coloured theme that follows the terminal. The
    // shared `options()` is `plain: true` (most rows here do not care about
    // colour), which would make this row vacuous.
    let mut opts = options();
    opts.plain = false;
    // `auto` is what an unconfigured install uses - the spelling the old
    // literal-`"system"` check missed.
    opts.theme = "auto".to_string();
    let mut chat = crate::chat::Chat::new(
        opts,
        std::sync::Arc::new(lca_tui::engine::keybindings::KeybindingsManager::default()),
    );
    let mut screen = Screen::Main(MainScreenRenderer::new());
    let (input_tx, _input_rx) = std::sync::mpsc::channel::<String>();
    let (resize_tx, _resize_rx) = std::sync::mpsc::channel::<()>();
    let mut aborted = false;

    // Solarized Dark's background, as `OSC 11 ; rgb:00/2b/36 ST`.
    let reply = "\x1b]11;rgb:00/2b/36\x1b\\";
    let _ = handle_input(
        reply,
        &mut chat,
        &mut screen,
        &mut term,
        &input_tx,
        &resize_tx,
        None,
        &None,
        &mut aborted,
    );

    // The palette moved to the probed background's family, not the scheme base.
    let band = chat.theme.palette.get(crate::theme::Role::UserMessageBg);
    let crate::theme::Color::Rgb(r, g, b) = band else {
        panic!("the user band is a concrete colour after the reply: {band:?}");
    };
    assert_ne!(
        (r, g, b),
        (0x34, 0x35, 0x41),
        "the scheme base's #343541 is what the old code painted; a probed \
         background must replace it"
    );
    assert_ne!(
        (r, g, b),
        (0x00, 0x2b, 0x36),
        "and the band is not the probed background itself (the invisible-card bug)"
    );
    // The reference background is genuinely Solarized: the derived band is
    // stepped lighter than it.
    let probed = RgbColor {
        r: 0x00,
        g: 0x2b,
        b: 0x36,
    };
    assert!(
        crate::theme::panels::oklab_lightness(RgbColor { r, g, b })
            > crate::theme::panels::oklab_lightness(probed),
        "a dark background steps the panel lighter"
    );
}

// ---------------------------------------------------------------------------
// gh #403 gap 2: one formatter for the resume command.
//
// The fullscreen-exit hint (gh #82) named an id; the exit notice needs the whole
// *pasteable command*. Two `format!`s would drift, so both call
// `resume_command`. These rows pin the string and, crucially, that a relocated
// session dir comes back **shell-quoted and parseable** - the third honesty gate.
// ---------------------------------------------------------------------------

// Verifies: gh #403 gap 2 - the default case names the real flag and stays clean.
#[test]
fn resume_command_uses_the_real_flag_for_the_default_home() {
    assert_eq!(
        crate::resume_command::resume_command("abc123", None),
        "lca --resume abc123",
        "the common command carries no quoting noise"
    );
}

// Verifies: gh #403 gap 2 - **a flag-looking argument is quoted.** A session id
// or path that begins with `-` would otherwise be read as an option by the very
// tool the hint tells the user to run, turning a correct hint into a broken one.
#[test]
fn resume_command_quotes_an_argument_that_looks_like_a_flag() {
    let command = crate::resume_command::resume_command("-rf", None);
    assert!(
        !command.ends_with(" -rf"),
        "an id beginning with `-` is quoted so it is not parsed as a flag: {command}"
    );
}

// Verifies: gh #403 gap 2 - **a relocated session dir is quoted.** `--resume`
// resolves against the current data home, so a session carried in from elsewhere
// needs `--session <path>`; a path with a space must not split into two words.
#[test]
fn resume_command_quotes_a_relocated_session_directory() {
    let dir = std::path::Path::new("/tmp/my sessions/2026-10-23-1");
    let command = crate::resume_command::resume_command("abc123", Some(dir));
    assert!(
        command.starts_with("lca --session "),
        "a relocated session names its path: {command}"
    );
    // The quoting dialect is platform-dependent on purpose: `cmd`/PowerShell do
    // not treat `'` the way POSIX shells do, so Windows gets `"` with `"` doubled.
    // Assert the *behaviour* (the space is inside quotes), not one dialect - a row
    // keyed to `'` passed on Linux and failed on the Windows runner, which is the
    // raw-path-comparison lesson in a new place.
    let quoted = if cfg!(windows) { '"' } else { '\'' };
    assert!(
        command.contains(quoted),
        "the path with a space is quoted for this platform: {command}"
    );
    assert!(
        command.contains("my sessions"),
        "and the space survives inside the quotes: {command}"
    );
    assert!(
        !command.contains("lca --resume"),
        "the id alone would resolve against the wrong home: {command}"
    );
}

// Verifies: gh #403 gap 2 - **the quoted command actually parses.** Asserting a
// quote character is present proves nothing; this feeds the printed line to a
// real shell (`sh -c`) and checks the argument list the shell reconstructs.
#[cfg(unix)]
#[test]
fn the_quoted_command_round_trips_through_a_real_shell() {
    let awkward = "/tmp/lca resume test-403/space and 'quote' here";
    let dir = std::path::Path::new(awkward);
    let command = crate::resume_command::resume_command("abc123", Some(dir));
    // Reconstruct the argv `sh` would build, without running `lca`: the command
    // word is replaced by `printf` so nothing executes.
    let probe = command.replacen("lca --session ", "printf '%s' ", 1);
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(&probe)
        .output()
        .expect("run sh");
    assert!(
        output.status.success(),
        "the printed command parses: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        awkward,
        "and the shell reconstructs the path byte-for-byte: {command}"
    );
}

// Verifies: gh #403 gap 2 - **gate 1: a non-TTY stdout says nothing.** A piped or
// headless run must gain zero bytes; scripted users parse this output.
#[test]
fn exit_resume_hint_is_silent_without_a_tty() {
    assert_eq!(
        crate::resume_command::exit_resume_hint("abc123", None, false),
        None,
        "a piped run gets no hint, so its output is byte-identical to before"
    );
    assert_eq!(
        crate::resume_command::exit_resume_hint("abc123", None, true),
        Some("resume with: lca --resume abc123".to_string())
    );
}

// Verifies: gh #403 gap 2 - **gate 2: no id, no hint.** A run that never reached
// `session-start` wrote nothing, so there is nothing to advertise.
#[test]
fn exit_resume_hint_is_silent_without_a_saved_session() {
    assert_eq!(
        crate::resume_command::exit_resume_hint("", None, true),
        None,
        "no session id means no saved file to resume"
    );
}

// Verifies: gh #403 gap 2 - the exit notice and the fullscreen hint name the same
// command, which is what makes "one formatter" observable rather than aspirational.
#[test]
fn both_hints_share_the_same_command_string() {
    let tuning = crate::state::DisplayTuning {
        fullscreen_exit_output: "resume-hint".to_string(),
        ..crate::state::DisplayTuning::default()
    };
    let fullscreen = super::exit_hint(&tuning, "abc123").expect("the gh #82 hint");
    let on_exit =
        crate::resume_command::exit_resume_hint("abc123", None, true).expect("the gh #403 hint");
    assert!(
        fullscreen.contains(&crate::resume_command::resume_command("abc123", None)),
        "the fullscreen hint embeds the shared command: {fullscreen}"
    );
    assert!(
        on_exit.contains(&crate::resume_command::resume_command("abc123", None)),
        "so does the exit hint: {on_exit}"
    );
}
