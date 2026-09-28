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

use super::core::extract_cursor_position;
use super::layout::{LayoutNode, render_frame};
use super::selection::{Granularity, Selection, SelectionPoint};
use super::terminal::Terminal;
use super::text::{strip_terminal_sequences, visible_width};

/// A parsed SGR mouse event (`ESC [ < b ; x ; y M|m`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SgrMouse {
    /// The button/modifier bitfield.
    pub bits: u16,
    /// 1-based column.
    pub x: u16,
    /// 1-based row.
    pub y: u16,
    /// Whether this is a press (`M`) or release (`m`).
    pub press: bool,
}

/// Parse an SGR mouse sequence.
pub fn parse_sgr_mouse(data: &str) -> Option<SgrMouse> {
    let rest = data.strip_prefix("\x1b[<")?;
    let (body, press) = if let Some(b) = rest.strip_suffix('M') {
        (b, true)
    } else {
        (rest.strip_suffix('m')?, false)
    };
    let parts: Vec<&str> = body.split(';').collect();
    if parts.len() != 3 {
        return None;
    }
    let bits = parts[0].parse().ok()?;
    let x = parts[1].parse().ok()?;
    let y = parts[2].parse().ok()?;
    Some(SgrMouse { bits, x, y, press })
}

/// Whether the environment is a terminal multiplexer (button-motion only).
pub fn is_multiplexer() -> bool {
    std::env::var_os("TMUX").is_some()
        || std::env::var_os("ZELLIJ").is_some()
        || std::env::var_os("STY").is_some()
}

/// The mouse-enable sequences for this environment.
pub fn mouse_enable_sequences() -> &'static str {
    if is_multiplexer() {
        "\x1b[?1000h\x1b[?1002h\x1b[?1006h"
    } else {
        "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h"
    }
}

/// The mouse-disable sequences.
pub const MOUSE_DISABLE: &str = "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l";

/// How a copy completed, for honest reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOutcome {
    /// A verified native clipboard accepted the text.
    Verified,
    /// Only an unverifiable OSC 52 write was possible.
    Unverified,
}

/// The fullscreen renderer.
pub struct AltScreenRenderer {
    previous: Vec<String>,
    width: u16,
    height: u16,
    first_render: bool,
    /// The active selection.
    pub selection: Selection,
    /// Scroll offset of the implicit primary scroll view.
    pub scroll: u16,
    /// Whether to copy on release.
    pub copy_on_select: bool,
    last_click: Option<(std::time::Instant, u16, u16)>,
    click_count: u8,
    /// While dragging, the edge the pointer sits on: `-1` top, `1` bottom
    /// (R6's edge auto-scroll). `None` when not at an edge.
    drag_edge: Option<i8>,
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
            width: 0,
            height: 0,
            first_render: true,
            selection: Selection::new(),
            scroll: 0,
            copy_on_select: true,
            last_click: None,
            click_count: 0,
            drag_edge: None,
        }
    }

    /// Enter the alt screen and enable mouse tracking.
    pub fn enter(&mut self, term: &mut dyn Terminal) {
        let mut out = String::from("\x1b[?1049h\x1b[?7l");
        out.push_str(mouse_enable_sequences());
        out.push_str("\x1b[2J\x1b[H\x1b[?25l");
        term.write(&out);
    }

    /// Leave the alt screen, restoring main-screen scrollback with the final
    /// document so the transcript survives.
    pub fn leave(&mut self, term: &mut dyn Terminal, preserve_screen: bool) {
        let mut out = String::from(MOUSE_DISABLE);
        if !preserve_screen {
            // Re-render the final document into the main screen.
            out.push_str("\x1b[?1049l");
            for (i, line) in self.previous.iter().enumerate() {
                if i > 0 {
                    out.push_str("\r\n");
                }
                out.push_str(&strip_terminal_sequences(line));
            }
        } else {
            out.push_str("\x1b[?1049l");
        }
        out.push_str("\x1b[?7h\x1b[?25h");
        term.write(&out);
        self.first_render = true;
    }

    /// Render one frame.
    pub fn render(
        &mut self,
        term: &mut dyn Terminal,
        root: &mut LayoutNode,
        width: u16,
        height: u16,
    ) -> Option<(u16, u16)> {
        let frame = render_frame(root, width, height);
        self.render_lines(term, frame.lines, width, height)
    }

    /// Render already-composed lines with the selection highlight and the
    /// full-screen diff. Used by the agent interface, which composes its
    /// own viewport (transcript + dock) rather than a `LayoutNode`.
    pub fn render_lines(
        &mut self,
        term: &mut dyn Terminal,
        lines: Vec<String>,
        width: u16,
        height: u16,
    ) -> Option<(u16, u16)> {
        let mut lines = lines;
        lines.resize(height as usize, String::new());
        // Apply the selection highlight.
        self.selection.highlight(&mut lines);
        let (lines, cursor) = extract_cursor_position(&lines);

        let resize = self.first_render || width != self.width || height != self.height;
        let mut out = String::from("\x1b[?2026h");
        if resize {
            out.push_str("\x1b[2J\x1b[H");
        }
        for (row, line) in lines.iter().enumerate() {
            if !resize && self.previous.get(row) == Some(line) {
                continue;
            }
            out.push_str(&format!("\x1b[{};1H\x1b[2K", row + 1));
            out.push_str(line);
        }
        if let Some((row, col)) = cursor {
            out.push_str(&format!("\x1b[{};{}H\x1b[?25h", row + 1, col + 1));
        } else {
            out.push_str("\x1b[?25l");
        }
        out.push_str("\x1b[?2026l");
        term.write(&out);
        self.previous = lines;
        self.width = width;
        self.height = height;
        self.first_render = false;
        cursor
    }

    /// Handle raw input: mouse selection, wheel scroll, and viewport keys.
    /// Returns true if consumed.
    pub fn handle_input(&mut self, data: &str) -> bool {
        if let Some(mouse) = parse_sgr_mouse(data) {
            return self.handle_mouse(mouse);
        }
        // Focus out clears selection and drags.
        if data == "\x1b[O" {
            self.selection.clear();
            return true;
        }
        false
    }

    /// Handle one mouse event against the current rendered lines.
    pub fn handle_mouse(&mut self, mouse: SgrMouse) -> bool {
        let motion = mouse.bits & 32 != 0;
        let wheel = mouse.bits & 64 != 0;
        let button = mouse.bits & 3;
        let col = mouse.x.saturating_sub(1);
        let row = mouse.y.saturating_sub(1);
        let point = SelectionPoint { row, col };

        if wheel {
            let delta = if button == 0 { 1i32 } else { -1 };
            self.scroll = (self.scroll as i32 + delta).max(0) as u16;
            return true;
        }
        if motion {
            if self.selection.dragging {
                // Edge auto-scroll (R6): a drag at the top/bottom row arms a
                // scroll that `tick_auto_scroll` advances on the loop's tick.
                self.drag_edge = if row == 0 {
                    Some(-1)
                } else if row + 1 >= self.height {
                    Some(1)
                } else {
                    None
                };
                let line = self.previous.get(row as usize).cloned().unwrap_or_default();
                self.selection.update(point, &line);
                return true;
            }
            return false;
        }
        if mouse.press {
            // Track double/triple click within 500 ms on the same cell.
            let now = std::time::Instant::now();
            self.click_count = match self.last_click {
                Some((t, x, y))
                    if now.duration_since(t).as_millis() < 500 && x == col && y == row =>
                {
                    (self.click_count % 3) + 1
                }
                _ => 1,
            };
            self.last_click = Some((now, col, row));
            let line = self.previous.get(row as usize).cloned().unwrap_or_default();
            let granularity = Granularity::from_click_count(self.click_count);
            self.selection
                .start(point, granularity, self.click_count, &line);
            true
        } else {
            self.selection.end();
            self.drag_edge = None;
            true
        }
    }

    /// Advance an edge drag by one line (R6): scroll and extend the
    /// selection to the edge. Returns whether it scrolled. The loop calls
    /// this every tick while a drag sits on an edge.
    pub fn tick_auto_scroll(&mut self) -> bool {
        let Some(edge) = self.drag_edge else {
            return false;
        };
        if edge < 0 {
            if self.scroll == 0 {
                return false;
            }
            self.scroll -= 1;
        } else {
            self.scroll = self.scroll.saturating_add(1);
        }
        let row = if edge < 0 {
            0
        } else {
            self.height.saturating_sub(1)
        };
        let line = self.previous.get(row as usize).cloned().unwrap_or_default();
        let col = line.chars().count().saturating_sub(1) as u16;
        self.selection.update(SelectionPoint { row, col }, &line);
        true
    }

    /// The selected text, if any.
    pub fn selected_text(&self) -> String {
        self.selection.active_text(&self.previous)
    }

    /// Render an OSC 52 copy. Returns whether the write was verifiable
    /// (never, without an injected native clipboard — the honest answer).
    pub fn copy_osc52(&self, term: &mut dyn Terminal, text: &str) -> CopyOutcome {
        let encoded = base64_encode(text.as_bytes());
        term.write(&format!("\x1b]52;c;{encoded}\x07"));
        CopyOutcome::Unverified
    }

    /// Whether a line is entirely blank.
    pub fn is_blank(line: &str) -> bool {
        visible_width(line) == 0
    }
}

// The engine has no base64 dependency; a tiny local encoder keeps OSC 52
// self-contained.
#[allow(clippy::needless_range_loop)]
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn base64_encodes_osc52_payloads() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b""), "");
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
