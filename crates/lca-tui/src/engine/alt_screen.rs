//! Fullscreen (alt-screen) renderer with application-owned selection,
//! ported from pi's `packages/tui/src/tui-alt-screen.ts`
//! (`pi-tui-re/src_re/tui-widgets/tui-alt-screen.md`).
//!
//! This is the renderer that fixes LCA issue #2: in fullscreen mode the
//! application owns text selection (drag/double-click/triple-click,
//! copy-on-release), while the regular main-screen mode leaves mouse
//! tracking off so the terminal's own selection and scrollback keep
//! working. Mouse mode is multiplexer-aware: tmux/zellij/screen get
//! button-motion tracking (1000+1002+1006), others also get all-motion
//! (1003).
//!
//! Not ported (documented skips): the search overlay, kitty image cache and
//! placement-only redraws, the WezTerm image/EL workaround, and the
//! scroll-to-end indicator. The transcript-preservation handoff on exit is
//! kept.
//!
//! This file is the barrel: the renderer's owned state, the frame
//! (`enter`/`render_lines`), and the module declarations. The behaviour sits
//! beside the rows that already named it (gh #572) - the `#[path]` test
//! siblings below *are* the partition:
//!
//! - `mouse.rs` - SGR parsing, the terminal's mouse/focus modes, the wheel model;
//! - `input.rs` - what an event means: selection, drag, the click run, focus loss;
//! - `drag.rs` - the wheel's scroll step, edge auto-scroll, the scrollbar grab;
//! - `exit.rs` - the handoff back to the scrollback and its two strippers.

pub(crate) mod image_blocks;
use image_blocks::expand_for_image_blocks;

use super::selection::Selection;
use super::terminal::Terminal;

mod drag;
mod exit;
mod input;
mod mouse;

pub use mouse::{
    FOCUS_IN, FOCUS_OUT, MOUSE_DISABLE, SgrMouse, WheelAccel, is_multiplexer,
    mouse_enable_sequences, parse_sgr_mouse,
};

pub use right_click::right_click_paste_allowed;
mod copy_out;
mod right_click;
pub use copy_out::CopyOutcome;

/// The fullscreen renderer.
pub struct AltScreenRenderer {
    previous: Vec<String>,
    /// The caret from the last frame: a caret that moved with no text change
    /// still repaints its cell (R5).
    last_cursor: Option<(u16, u16)>,
    width: u16,
    height: u16,
    first_render: bool,
    /// The active selection.
    pub selection: Selection,
    /// Scroll offset of the implicit primary scroll view.
    pub scroll: u16,
    /// Whether to copy on release.
    pub copy_on_select: bool,
    /// Fullscreen wheel lines per event (gh #82, pi's
    /// `fullscreenWheelScrollLines` without the platform sniff): the
    /// fixed step for a numeric setting. The floor keeps a zero away.
    pub wheel_lines: u8,
    /// Whether the wheel accelerates with velocity (gh #206, pi's
    /// `"auto"`): an isolated notch moves one line, a fast spin up to
    /// six. The host syncs this from the setting and resets the
    /// accelerator whenever it changes.
    pub wheel_auto: bool,
    /// The velocity state for `wheel_auto` (gh #206).
    wheel_accel: WheelAccel,
    /// The clock `wheel_auto` measures gaps against (gh #206); the first event
    /// starts it.
    wheel_epoch: Option<std::time::Instant>,
    last_click: Option<(std::time::Instant, u16, u16)>,
    click_count: u8,
    /// While dragging, the edge the pointer sits on: `-1` top, `1` bottom (R6's
    /// edge auto-scroll); `None` when not at an edge.
    drag_edge: Option<i8>,
    /// The cell a press landed on, for click-vs-drag (R6's link open).
    press_cell: Option<(u16, u16)>,
    /// The cell a completed single click landed on (gh #11's thinking
    /// toggle): press and release on the same cell with no movement and
    /// no multi-click. The loop takes it and hit-tests the transcript.
    clicked_cell: Option<(u16, u16)>,
    /// The OSC-8 URL a completed click landed on, for the loop to open.
    clicked_link: Option<String>,
    /// Right-button paste state (gh #374 gap 4): `(requested, allowed)`, both
    /// owned and documented by [`self::right_click`].
    pub(super) right_click: (bool, bool),
    /// Whether the pointer hovers the scrollbar (gh #164); an overlay covering the
    /// viewport clears it (pi's `stopScrollbarHover`).
    scrollbar_hover: bool,
    /// Whether a scrollbar drag is active (gh #164): motion and release belong to
    /// the drag, never to selection. The grab math lives with the geometry in the
    /// interface; the renderer only suppresses.
    scrollbar_drag: bool,
    /// The whole transcript this session has, as of the last frame (gh #374 gap
    /// 1): what [`leave`](Self::leave) hands back to the scrollback.
    ///
    /// Not the rendered frame - that is `previous`, and it holds exactly `height`
    /// rows. The host owns the document (it is the thing that can compose the
    /// transcript above the viewport), so it hands it over every frame and the
    /// renderer keeps the latest one for the exit.
    document: Vec<String>,
}

impl Default for AltScreenRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl AltScreenRenderer {
    /// A fresh renderer.
    pub fn new() -> Self {
        Self {
            previous: Vec::new(),
            last_cursor: None,
            width: 0,
            height: 0,
            first_render: true,
            selection: Selection::new(),
            scroll: 0,
            copy_on_select: true,
            wheel_lines: 1,
            wheel_auto: false,
            wheel_accel: WheelAccel::new(),
            wheel_epoch: None,
            last_click: None,
            click_count: 0,
            drag_edge: None,
            press_cell: None,
            clicked_cell: None,
            clicked_link: None,
            right_click: (false, false),
            scrollbar_hover: false,
            scrollbar_drag: false,
            document: Vec::new(),
        }
    }

    /// Enter the alt screen and enable mouse tracking.
    pub fn enter(&mut self, term: &mut dyn Terminal) {
        let mut out = String::from("\x1b[?1049h\x1b[?7l");
        out.push_str(mouse_enable_sequences());
        out.push_str("\x1b[2J\x1b[H\x1b[?25l");
        term.write(&out);
    }

    /// The lines from the last render.
    pub fn previous(&self) -> &[String] {
        &self.previous
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::selection::{Granularity, SelectionPoint};
    use crate::engine::terminal::FakeTerminal;

    // Verifies: FR-UI-24 (R5) - a caret-only move repaints the caret's cell
    // (the row is not skipped as "unchanged") and positions the column.
    #[test]
    fn a_caret_only_move_repaints_its_cell() {
        use crate::engine::core::CURSOR_MARKER;
        let mut term = FakeTerminal::new(20, 10);
        let mut renderer = AltScreenRenderer::new();
        renderer.render_lines(
            &mut term,
            vec![format!("ab{CURSOR_MARKER} "), "footer".into()],
            20,
            10,
        );
        let _ = term.take_output();
        renderer.render_lines(
            &mut term,
            vec![format!("ab {CURSOR_MARKER}"), "footer".into()],
            20,
            10,
        );
        let out = term.take_output();
        assert!(out.contains("\x1b[1;1H"), "row 1 repainted: {out:?}");
        assert!(out.contains("\x1b[1;4H"), "caret column placed: {out:?}");
        assert!(
            !out.contains("footer"),
            "the untouched row stays untouched: {out:?}"
        );
    }

    // Verifies: S3 (issue #7) - the alt-screen renderer enables exactly pi's mouse set
    // on entry (app-owned selection) and disables every mode on exit. In contrast to
    // default main-screen mode, mouse capture is explicitly opt-in here.
    #[test]
    fn alt_screen_emits_exactly_the_pi_mouse_sequences() {
        let mut term = FakeTerminal::new(80, 24);
        let mut renderer = AltScreenRenderer::new();
        renderer.enter(&mut term);
        let entered = term.take_output();
        assert!(
            entered.contains(mouse_enable_sequences()),
            "entry enables the environment's mouse set:\n{entered:?}"
        );
        assert!(entered.contains("\x1b[?1006h"), "SGR encoding is on");
        assert!(entered.contains("\x1b[?1049h"), "the alt screen is entered");

        let mut term = FakeTerminal::new(80, 24);
        renderer.leave(&mut term, true);
        let left = term.take_output();
        assert!(
            left.contains(MOUSE_DISABLE),
            "every enabled mouse mode is disabled on exit:\n{left:?}"
        );
        assert!(left.contains("\x1b[?1049l"), "the alt screen is left");
    }

    #[test]
    fn parses_sgr_mouse_press_release_and_wheel() {
        let press = parse_sgr_mouse("\x1b[<0;10;5M").unwrap();
        assert_eq!(
            (press.bits, press.x, press.y, press.press),
            (0, 10, 5, true)
        );
        let release = parse_sgr_mouse("\x1b[<0;10;5m").unwrap();
        assert!(!release.press);
        let wheel = parse_sgr_mouse("\x1b[<64;3;4M").unwrap();
        assert_eq!(wheel.bits & 64, 64);
    }

    #[test]
    fn double_click_selects_a_word_and_copies() {
        let mut r = AltScreenRenderer::new();
        r.previous = vec!["hello /path/to-file.txt world".to_string()];
        // Two clicks on the same cell within the window => word granularity.
        let ev = SgrMouse {
            bits: 0,
            x: 12,
            y: 1,
            press: true,
        };
        r.handle_mouse(ev);
        r.handle_mouse(ev);
        assert_eq!(r.selected_text(), "/path/to-file.txt");
    }

    #[test]
    fn drag_extends_a_char_selection() {
        let mut r = AltScreenRenderer::new();
        r.previous = vec!["hello world".to_string()];
        r.handle_mouse(SgrMouse {
            bits: 0,
            x: 1,
            y: 1,
            press: true,
        });
        r.handle_mouse(SgrMouse {
            bits: 32, // motion
            x: 6,
            y: 1,
            press: true,
        });
        r.handle_mouse(SgrMouse {
            bits: 0,
            x: 6,
            y: 1,
            press: false,
        });
        assert_eq!(r.selected_text(), "hello");
    }

    #[test]
    fn wheel_scrolls() {
        let mut r = AltScreenRenderer::new();
        r.handle_mouse(SgrMouse {
            bits: 64,
            x: 1,
            y: 1,
            press: true,
        });
        assert_eq!(r.scroll, 1);
    }

    // Verifies: gh #82 - the wheel moves `wheel_lines` rows per event
    // (pi's `fullscreenWheelScrollLines` without the platform sniff).
    #[test]
    fn wheel_moves_configured_lines() {
        let mut r = AltScreenRenderer::new();
        r.wheel_lines = 6;
        r.handle_mouse(SgrMouse {
            bits: 64,
            x: 1,
            y: 1,
            press: true,
        });
        assert_eq!(r.scroll, 6);
        // Down (bit 0 set) walks back.
        r.handle_mouse(SgrMouse {
            bits: 65,
            x: 1,
            y: 1,
            press: true,
        });
        assert_eq!(r.scroll, 0);
    }

    // Verifies: R6 - a click on an OSC-8 link yields its URL.
    #[test]
    fn a_click_on_a_link_yields_the_url() {
        let mut r = AltScreenRenderer::new();
        r.previous = vec!["\x1b]8;;https://pi.dev\x07link\x1b]8;;\x07 here".to_string()];
        r.width = 40;
        r.height = 1;
        // Press and release on the same cell (column 1).
        r.handle_mouse(SgrMouse {
            bits: 0,
            x: 2,
            y: 1,
            press: true,
        });
        r.handle_mouse(SgrMouse {
            bits: 0,
            x: 2,
            y: 1,
            press: false,
        });
        assert_eq!(r.take_clicked_link().as_deref(), Some("https://pi.dev"));
        // A drag does not open a link.
        r.handle_mouse(SgrMouse {
            bits: 0,
            x: 2,
            y: 1,
            press: true,
        });
        r.handle_mouse(SgrMouse {
            bits: 32, // motion
            x: 5,
            y: 1,
            press: true,
        });
        r.handle_mouse(SgrMouse {
            bits: 0,
            x: 5,
            y: 1,
            press: false,
        });
        assert!(r.take_clicked_link().is_none());
    }

    // Verifies: R6 - a drag on a viewport edge auto-scrolls and extends.
    #[test]
    fn a_drag_at_the_bottom_edge_auto_scrolls() {
        let mut r = AltScreenRenderer::new();
        r.previous = (0..10).map(|i| format!("line {i}")).collect();
        r.width = 20;
        r.height = 10;
        r.selection.start(
            SelectionPoint { row: 5, col: 0 },
            Granularity::Char,
            1,
            "line 5",
        );
        // Motion at the bottom row (y=10, 1-based) arms the scroll.
        r.handle_mouse(SgrMouse {
            bits: 32, // motion
            x: 1,
            y: 10,
            press: true,
        });
        assert!(r.tick_auto_scroll());
        assert_eq!(r.scroll, 1);
        // A release clears the edge.
        r.handle_mouse(SgrMouse {
            bits: 0,
            x: 1,
            y: 10,
            press: false,
        });
        assert!(!r.tick_auto_scroll());
    }
}

#[cfg(test)]
mod scrollbar_tests {
    use super::*;

    // Verifies: gh #164 - while a scrollbar drag is active the renderer
    // consumes motion without touching the selection.
    #[test]
    fn a_scrollbar_drag_suppresses_selection() {
        let mut renderer = AltScreenRenderer::new();
        renderer.previous = vec!["line one".to_string()];
        renderer.begin_scrollbar_drag();
        assert!(renderer.scrollbar_dragging());
        assert!(renderer.handle_mouse(SgrMouse {
            bits: 32,
            x: 79,
            y: 2,
            press: true,
        }));
        assert!(!renderer.selection.is_active(), "no selection starts");
        assert!(renderer.selected_text().is_empty());
        renderer.end_scrollbar_drag();
        assert!(!renderer.scrollbar_dragging());
    }

    // Verifies: gh #164 - hover is a flag round-trip; selection still
    // starts when no drag is active.
    #[test]
    fn hover_reports_without_side_effects() {
        let mut renderer = AltScreenRenderer::new();
        renderer.set_scrollbar_hover(true);
        assert!(renderer.scrollbar_hovered());
        renderer.set_scrollbar_hover(false);
        assert!(!renderer.scrollbar_hovered());
    }
}

#[cfg(test)]
mod wheel_tests {
    use super::*;

    // Verifies: gh #206 - an isolated notch moves one line in auto
    // mode, and sub-notch bursts (under 5 ms apart) never accelerate.
    #[test]
    fn auto_isolated_and_burst_notches_move_one_line() {
        let mut accel = WheelAccel::new();
        assert_eq!(accel.next(-1, 1000), 1);
        assert_eq!(accel.next(-1, 1004), 1, "4 ms later: the same notch");
        assert_eq!(accel.next(-1, 1300), 1, "the pause ended the gesture");
    }

    // Verifies: gh #206 - fast spins scale toward the six-line cap; a
    // direction flip or a long pause resets to one line.
    #[test]
    fn auto_velocity_scales_and_resets() {
        let mut accel = WheelAccel::new();
        assert_eq!(accel.next(-1, 0), 1);
        assert_eq!(accel.next(-1, 50), 2, "50 ms gaps double");
        assert_eq!(accel.next(-1, 100), 2, "steady gaps hold");
        assert_eq!(accel.next(-1, 110), 3, "10 ms gaps climb");
        assert_eq!(accel.next(-1, 120), 5, "the average keeps falling");
        assert_eq!(accel.next(-1, 130), 6, "the six-line cap holds");
        assert_eq!(accel.next(1, 140), 1, "a flip resets");
        assert_eq!(accel.next(1, 500), 1, "a pause resets");
    }

    // Verifies: gh #206 - a numeric setting ignores velocity, and Alt
    // multiplies by five either way.
    #[test]
    fn fixed_lines_ignore_velocity_and_alt_multiplies() {
        let mut renderer = AltScreenRenderer::new();
        renderer.wheel_lines = 2;
        renderer.wheel_auto = false;
        assert_eq!(renderer.wheel_step(-1, false, 1000), 2);
        assert_eq!(
            renderer.wheel_step(-1, false, 1050),
            2,
            "no accel when fixed"
        );
        assert_eq!(renderer.wheel_step(-1, true, 1050), 10, "Alt ×5");
        renderer.wheel_auto = true;
        assert_eq!(
            renderer.wheel_step(-1, false, 2000),
            1,
            "auto starts at one"
        );
        assert_eq!(renderer.wheel_step(-1, false, 2050), 2, "auto accelerates");
        assert_eq!(renderer.wheel_step(-1, true, 2100), 10, "auto Alt ×5");
    }
}

// The gh #374 rows, split out for the 1,200-line ceiling (gaps 1-3).
#[cfg(test)]
#[path = "alt_screen/backward_drag_tests.rs"]
mod backward_drag_tests;
#[cfg(test)]
#[path = "alt_screen/exit_handoff_tests.rs"]
mod exit_handoff_tests;
#[cfg(test)]
#[path = "alt_screen/focus_tests.rs"]
mod focus_tests;
#[cfg(test)]
#[path = "alt_screen/right_click_tests.rs"]
mod right_click_tests;

#[cfg(test)]
#[path = "alt_screen/image_block_tests.rs"]
mod image_block_tests;
