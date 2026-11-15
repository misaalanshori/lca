//! Gh #600: a `xterm-256color` terminal keeps its palette.
//!
//! # The defect
//!
//! `ColorSupport` had three rungs but only two producers: a coloured theme gave
//! `Truecolor`, `ui.color = never` gave `None`. `Ansi16` existed and **no
//! session path ever produced it**, there was no 256 rung, and no colour-depth
//! detection existed anywhere in `crates/` (`COLORTERM` was never read). So a
//! `TERM=xterm-256color` session with `COLORTERM` unset was handed
//! `38;2;r;g;b` - bytes such a terminal either shows as garbage or snaps to its
//! own cube - and the only honest answer a user had was `never`: no colour at
//! all, which is the "plain theme on a capable terminal" outcome
//! `docs/terminal-setup.md` says should not happen.
//!
//! # Why these rows live out here
//!
//! The arithmetic rows sit beside their subjects (`lca-tui`'s `color_depth_tests`,
//! `lca-ui`'s `theme::depth`). These are the **seam** rows: they drive the
//! production path a session uses (`UiOptions` -> `Chat::new` -> `render`) from
//! outside both crates, so a change that keeps the units green while breaking the
//! wiring is caught here. That is the class the issue is in - the rungs existed,
//! the ladder did not.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lca_tui::engine::colors::ColorDepth;
use lca_tui::engine::keybindings::KeybindingsManager;
use lca_ui::{Chat, UiOptions};

/// A chat whose options name a rung - exactly what `tui/commands.rs` builds from
/// `current_color_depth`.
fn chat_at(depth: ColorDepth, plain: bool) -> Chat {
    Chat::new(
        UiOptions {
            prompt_slot: Default::default(),
            dialog_slot: Default::default(),
            pending_models: None,
            model_label: Arc::new(Mutex::new("fake/faux-1".to_string())),
            context_window: Arc::new(std::sync::Mutex::new(0)),
            thinking: Arc::new(Mutex::new(None)),
            theme: "dark".to_string(),
            theme_extra_dirs: Vec::new(),
            themes: lca_ui::theme::THEMES
                .iter()
                .map(|s| s.to_string())
                .collect(),
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
            plain,
            color_depth: depth,
            invoke_command: Arc::new(|_, _| lca_protocol::CommandEffect::None),
            slash_commands: Vec::new(),
            models: Vec::new(),
            workspace: PathBuf::from("."),
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
            hooks: lca_ui::UiHooks::default(),
            fullscreen: true,
            follow_up_mode: lca_protocol::QueueMode::OneAtATime,
        },
        Arc::new(KeybindingsManager::new()),
    )
}

/// A document with the three things a user actually sees painted.
fn painted_document(depth: ColorDepth, plain: bool) -> String {
    let mut chat = chat_at(depth, plain);
    chat.transcript.push_user("hello there");
    chat.transcript.start_tool("bash", "echo hi");
    chat.transcript.finish_tool_status(lca_ui::ToolStatus::Ok);
    chat.say("a notice");
    chat.render(80).join("\n")
}

/// The kitty-free SGR bodies in a frame, so an assertion can count spellings.
fn sgr_bodies(frame: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = frame;
    while let Some(at) = rest.find("\x1b[") {
        let tail = &rest[at + 2..];
        let Some(end) = tail.find('m') else {
            break;
        };
        out.push(tail[..end].to_string());
        rest = &tail[end..];
    }
    out
}

// Verifies: gh #600 row 1 + 4 - **a 256-colour session paints indices, and never
// the bytes it cannot render.** The frame carries `38;5`/`48;5` for the band,
// the tool card and the notice, and **no** `38;2`/`48;2` anywhere. Red on the
// pre-cycle tree: the same document was full of 24-bit SGR.
#[test]
fn an_ansi256_session_paints_indices_and_never_24_bit_bytes() {
    let frame = painted_document(ColorDepth::Ansi256, false);
    let bodies = sgr_bodies(&frame);
    assert!(
        bodies
            .iter()
            .any(|body| body.starts_with("38;5;") || body.contains(";38;5;")),
        "a foreground index is painted: {bodies:?}"
    );
    assert!(
        bodies
            .iter()
            .any(|body| body.starts_with("48;5;") || body.contains(";48;5;")),
        "and a background index: {bodies:?}"
    );
    assert!(
        !frame.contains("38;2;") && !frame.contains("48;2;"),
        "no 24-bit bytes at a 256-colour rung:\n{frame}"
    );
}

// Verifies: gh #600 - **the truecolor rung is unchanged.** The same document at
// `Truecolor` still carries the bytes and no index, so the tier did not cost the
// rung that already worked.
#[test]
fn a_truecolor_session_still_paints_bytes() {
    let frame = painted_document(ColorDepth::Truecolor, false);
    assert!(
        frame.contains("38;2;") || frame.contains("48;2;"),
        "24-bit SGR survives: {frame}"
    );
    assert!(
        !frame.contains("38;5;") && !frame.contains("48;5;"),
        "and no palette index leaks in: {frame}"
    );
}

// Verifies: gh #600 + FR-UI-5 - **`ui.color = never` beats every rung.** The
// ladder puts the user's statement above the terminal's facts and above the
// forcing variable, so a `never` session paints no SGR at any depth - including
// `LCA_COLOR_DEPTH=truecolor`'s answer.
#[test]
fn a_never_session_paints_nothing_at_any_depth() {
    for depth in [
        ColorDepth::Truecolor,
        ColorDepth::Ansi256,
        ColorDepth::Ansi16,
        ColorDepth::None,
    ] {
        let frame = painted_document(depth, true);
        assert!(
            !frame.contains("\x1b["),
            "a never session emits no SGR at all at {depth:?}:\n{frame}"
        );
    }
}

// Verifies: gh #600 - **the ladder maps every rung onto a theme that spells it.**
// The engine's `ColorDepth` and the theme's `ColorSupport` are two vocabularies
// for one decision, and a rung that mapped to the wrong one would paint the
// wrong depth while every unit row on either side stayed green.
#[test]
fn every_rung_maps_to_its_own_theme_rung() {
    use lca_ui::theme::ColorSupport;
    for (depth, expected) in [
        (ColorDepth::Truecolor, ColorSupport::Truecolor),
        (ColorDepth::Ansi256, ColorSupport::Ansi256),
        (ColorDepth::Ansi16, ColorSupport::Ansi16),
        (ColorDepth::None, ColorSupport::None),
    ] {
        assert_eq!(ColorSupport::for_depth(depth), expected, "depth {depth:?}");
        assert_eq!(
            expected.paints(),
            depth.paints(),
            "and the two `paints` answers agree at {depth:?}"
        );
    }
}

// Verifies: gh #600 - **the detection ladder's real-environment form is the pure
// one.** A row that only drove the parameterised function would not notice the
// environment form passing different facts (`TERM` vs `COLORTERM` swapped, or a
// missing `never`). This one reads the process environment, which the workspace
// pins to a known `TERM` in CI (`tmux-256color` or `xterm-256color`), and asserts
// the two answers agree.
#[test]
fn the_environment_form_agrees_with_the_pure_ladder() {
    let term = std::env::var("TERM").ok();
    let colorterm = std::env::var("COLORTERM").ok();
    let never = false;
    let pure = lca_tui::engine::colors::color_depth(term.as_deref(), colorterm.as_deref(), never);
    let environment = lca_tui::engine::colors::current_color_depth(never);
    assert_eq!(
        pure, environment,
        "the environment form must be the pure ladder over the same facts: \
         TERM={term:?} COLORTERM={colorterm:?}"
    );
}

// Verifies: gh #600 - **a piped session paints nothing, from the ladder's own
// facts.** `TERM` absent is rule 6; a pipe is the case where escapes corrupt
// output rather than decorate it, so "no TERM -> None" is a product rule and not
// only a table entry.
#[test]
fn no_term_at_all_is_the_none_rung() {
    assert_eq!(
        lca_tui::engine::colors::color_depth(None, Some("truecolor"), false),
        ColorDepth::Truecolor,
        "COLORTERM alone still raises (a terminal that says so is a terminal)"
    );
    assert_eq!(
        lca_tui::engine::colors::color_depth(None, None, false),
        ColorDepth::None,
        "and with no hint at all there is nothing to paint into"
    );
}
