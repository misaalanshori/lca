//! SGR attribute tracking across wrapped lines (split from `text.rs`, P5).

use super::extract_ansi_code;
use super::osc8::{
    ActiveHyperlink, format_osc8_close, format_osc8_hyperlink, parse_osc8_hyperlink,
};

// =============================================================================
// AnsiCodeTracker (RE doc §5)
// =============================================================================

/// Tracks active SGR attributes and the active OSC 8 hyperlink so styling
/// can be re-emitted across wrapped lines.
#[derive(Debug, Clone, Default)]
pub struct AnsiCodeTracker {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    blink: bool,
    inverse: bool,
    hidden: bool,
    strikethrough: bool,
    fg_color: Option<String>,
    bg_color: Option<String>,
    active_hyperlink: Option<ActiveHyperlink>,
}

impl AnsiCodeTracker {
    /// A fresh tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Process one ANSI/OSC sequence.
    pub fn process(&mut self, ansi_code: &str) {
        if let Some(link) = parse_osc8_hyperlink(ansi_code) {
            self.active_hyperlink = link;
            return;
        }
        if !ansi_code.ends_with('m') {
            return;
        }
        let Some(body) = ansi_code
            .strip_prefix("\x1b[")
            .and_then(|s| s.strip_suffix('m'))
        else {
            return;
        };
        if body.is_empty() || body == "0" {
            self.reset();
            return;
        }
        let parts: Vec<&str> = body.split(';').collect();
        let mut i = 0;
        while i < parts.len() {
            let Ok(code) = parts[i].parse::<u32>() else {
                i += 1;
                continue;
            };
            if code == 38 || code == 48 {
                if parts.get(i + 1) == Some(&"5") && parts.get(i + 2).is_some() {
                    let color = format!("{};{};{}", parts[i], parts[i + 1], parts[i + 2]);
                    if code == 38 {
                        self.fg_color = Some(color);
                    } else {
                        self.bg_color = Some(color);
                    }
                    i += 3;
                    continue;
                }
                if parts.get(i + 1) == Some(&"2") && parts.get(i + 4).is_some() {
                    let color = format!(
                        "{};{};{};{};{}",
                        parts[i],
                        parts[i + 1],
                        parts[i + 2],
                        parts[i + 3],
                        parts[i + 4]
                    );
                    if code == 38 {
                        self.fg_color = Some(color);
                    } else {
                        self.bg_color = Some(color);
                    }
                    i += 5;
                    continue;
                }
            }
            match code {
                0 => self.reset(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                5 => self.blink = true,
                7 => self.inverse = true,
                8 => self.hidden = true,
                9 => self.strikethrough = true,
                21 => self.bold = false,
                22 => {
                    self.bold = false;
                    self.dim = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                25 => self.blink = false,
                27 => self.inverse = false,
                28 => self.hidden = false,
                29 => self.strikethrough = false,
                39 => self.fg_color = None,
                49 => self.bg_color = None,
                30..=37 | 90..=97 => self.fg_color = Some(code.to_string()),
                40..=47 | 100..=107 => self.bg_color = Some(code.to_string()),
                _ => {}
            }
            i += 1;
        }
    }

    fn reset(&mut self) {
        self.bold = false;
        self.dim = false;
        self.italic = false;
        self.underline = false;
        self.blink = false;
        self.inverse = false;
        self.hidden = false;
        self.strikethrough = false;
        self.fg_color = None;
        self.bg_color = None;
        // SGR reset deliberately does not clear the hyperlink.
    }

    /// Clear all state for reuse.
    pub fn clear(&mut self) {
        self.reset();
        self.active_hyperlink = None;
    }

    /// Re-emit the complete active state (styles + hyperlink).
    pub fn active_codes(&self) -> String {
        let mut codes: Vec<String> = Vec::new();
        if self.bold {
            codes.push("1".into());
        }
        if self.dim {
            codes.push("2".into());
        }
        if self.italic {
            codes.push("3".into());
        }
        if self.underline {
            codes.push("4".into());
        }
        if self.blink {
            codes.push("5".into());
        }
        if self.inverse {
            codes.push("7".into());
        }
        if self.hidden {
            codes.push("8".into());
        }
        if self.strikethrough {
            codes.push("9".into());
        }
        if let Some(fg) = &self.fg_color {
            codes.push(fg.clone());
        }
        if let Some(bg) = &self.bg_color {
            codes.push(bg.clone());
        }
        let mut result = if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        };
        if let Some(link) = &self.active_hyperlink {
            result.push_str(&format_osc8_hyperlink(link));
        }
        result
    }

    /// The active background code alone (scrollbar cell surgery).
    pub fn active_background_code(&self) -> String {
        self.bg_color
            .as_ref()
            .map(|bg| format!("\x1b[{bg}m"))
            .unwrap_or_default()
    }

    /// Whether any state is active.
    pub fn has_active_codes(&self) -> bool {
        self.bold
            || self.dim
            || self.italic
            || self.underline
            || self.blink
            || self.inverse
            || self.hidden
            || self.strikethrough
            || self.fg_color.is_some()
            || self.bg_color.is_some()
            || self.active_hyperlink.is_some()
    }

    /// Reset only what bleeds at line end: underline off and the hyperlink
    /// close (re-opened on the next line).
    pub fn line_end_reset(&self) -> String {
        let mut result = String::new();
        if self.underline {
            result.push_str("\x1b[24m");
        }
        if let Some(link) = &self.active_hyperlink {
            result.push_str(&format_osc8_close(link.terminator));
        }
        result
    }
}

pub(super) fn update_tracker_from_text(text: &str, tracker: &mut AnsiCodeTracker) {
    let mut i = 0;
    while i < text.len() {
        if let Some((code, len)) = extract_ansi_code(text, i) {
            tracker.process(&code);
            i += len;
        } else {
            let Some(ch) = super::char_at(text, i) else {
                break;
            };
            i += ch.len_utf8();
        }
    }
}

/// Only the background color active at the end of an ANSI string.
pub fn get_active_background_ansi(text: &str) -> String {
    let mut tracker = AnsiCodeTracker::new();
    update_tracker_from_text(text, &mut tracker);
    tracker.active_background_code()
}
