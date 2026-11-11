//! The mouse protocol: SGR parsing, the terminal's mouse modes, the focus
//! markers, and the wheel's velocity model (gh #206).
//!
//! Split from the barrel for gate 11 (the 1,200-line ceiling); the code, its
//! comments and its rows are unchanged. This is the layer that only *parses*
//! input - `input.rs` decides what a parsed event means.

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
///
/// Includes `?1004h` (focus reporting), which is what makes a focus loss
/// *observable* at all (gh #374 gap 2). Without it the terminal never sends
/// `ESC [ O`, the handler below is dead, and a fix that guards it is dead with
/// it. pi enables exactly this in `ENABLE_BUTTON_MOTION_MOUSE` /
/// `ENABLE_ALL_MOTION_MOUSE` (`tui-alt-screen.ts:67-68`).
pub fn mouse_enable_sequences() -> &'static str {
    if is_multiplexer() {
        "\x1b[?1000h\x1b[?1002h\x1b[?1004h\x1b[?1006h"
    } else {
        "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1004h\x1b[?1006h"
    }
}

/// The focus reports this renderer consumes (gh #374 gap 2): what a terminal
/// sends when focus reporting (`?1004h`) is on and the window enters or leaves
/// the focused state.
pub const FOCUS_IN: &str = "\x1b[I";
/// See [`FOCUS_IN`].
pub const FOCUS_OUT: &str = "\x1b[O";

/// The mouse-disable sequences.
///
/// `?1004l` is included for the mirror reason [`mouse_enable_sequences`] carries
/// `?1004h`: leaving focus reporting on would keep delivering reports after the
/// interface has gone, where a shell prompt would see them as typing.
pub const MOUSE_DISABLE: &str = "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l";

/// Wheel velocity acceleration (gh #206, pi's `WheelScrollAccelerator`
/// from `wheel-scroll.ts` with LCA's fixed three-line `auto` replaced
/// by pi's own velocity curve): an isolated notch moves one line, while
/// a fast spin scales toward six lines per event. Sub-notch bursts
/// (under 5 ms apart - one physical notch, or a high-resolution
/// source) move one line each without accelerating; a pause past
/// 200 ms ends the gesture. Fractional lines carry over, so slow spins
/// stay smooth instead of stepping in whole lines.
#[derive(Debug, Clone)]
pub struct WheelAccel {
    last_ms: Option<u64>,
    last_dir: i8,
    average_gap: Option<f64>,
    carry: f64,
}

impl WheelAccel {
    /// A fresh accelerator.
    pub fn new() -> Self {
        Self {
            last_ms: None,
            last_dir: 0,
            average_gap: None,
            carry: 0.0,
        }
    }

    /// Forget any gesture (gh #206): the host resets when the setting
    /// changes, so a stale velocity never leaks across modes.
    pub fn reset(&mut self) {
        self.last_ms = None;
        self.last_dir = 0;
        self.average_gap = None;
        self.carry = 0.0;
    }

    /// Lines for a wheel event in `direction` (-1 up, +1 down) at
    /// `now_ms` (gh #206): pi's gap math, one line for an isolated
    /// notch up to six for a fast spin.
    pub fn next(&mut self, direction: i8, now_ms: u64) -> u16 {
        // Several events closer than this belong to one physical notch
        // (pi's Ghostty note: ~4 ms apart) or a high-resolution source.
        const BURST_GAP_MS: u64 = 5;
        // A pause longer than this ends a scroll gesture.
        const GESTURE_GAP_MS: u64 = 200;
        // The event gap that maps to one line per event; faster events
        // scale up proportionally.
        const REFERENCE_GAP_MS: f64 = 100.0;
        const MAX_AUTO_LINES: f64 = 6.0;
        let gap = now_ms.saturating_sub(self.last_ms.unwrap_or(now_ms));
        let same = direction == self.last_dir && self.last_ms.is_some() && gap <= GESTURE_GAP_MS;
        self.last_ms = Some(now_ms);
        self.last_dir = direction;
        if !same {
            self.average_gap = None;
            self.carry = 0.0;
            return 1;
        }
        if gap < BURST_GAP_MS {
            return 1;
        }
        self.average_gap = Some(match self.average_gap {
            Some(average) => (average + gap as f64) / 2.0,
            None => gap as f64,
        });
        let average = self.average_gap.unwrap_or(REFERENCE_GAP_MS).max(1.0);
        let lines = MAX_AUTO_LINES.min(REFERENCE_GAP_MS / average).max(1.0) + self.carry;
        let whole = lines.floor() as u16;
        self.carry = lines - f64::from(whole);
        whole.max(1)
    }
}

impl Default for WheelAccel {
    fn default() -> Self {
        Self::new()
    }
}
