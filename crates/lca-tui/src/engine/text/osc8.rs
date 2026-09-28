//! OSC 8 hyperlink parsing and formatting (split from `text.rs`, P5).

use super::extract_ansi_code;

// =============================================================================
// OSC 8 hyperlinks
// =============================================================================

/// The terminator an OSC 8 link was opened with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc8Terminator {
    /// BEL (`\x07`).
    Bel,
    /// String Terminator (`ESC \`).
    St,
}

impl Osc8Terminator {
    fn as_str(self) -> &'static str {
        match self {
            Osc8Terminator::Bel => "\x07",
            Osc8Terminator::St => "\x1b\\",
        }
    }
}

/// An active OSC 8 hyperlink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveHyperlink {
    /// Link parameters (before the first `;`).
    pub params: String,
    /// The URL.
    pub url: String,
    /// The terminator it was opened with (preserved on reopen).
    pub terminator: Osc8Terminator,
}

/// Parse an OSC 8 hyperlink sequence: `Some(Some(link))` open,
/// `Some(None)` close, `None` not an OSC 8 sequence.
pub fn parse_osc8_hyperlink(code: &str) -> Option<Option<ActiveHyperlink>> {
    let rest = code.strip_prefix("\x1b]8;")?;
    let (terminator, body) = if let Some(b) = rest.strip_suffix('\x07') {
        (Osc8Terminator::Bel, b)
    } else {
        (Osc8Terminator::St, rest.strip_suffix("\x1b\\")?)
    };
    let (params, url) = body.split_once(';')?;
    if url.is_empty() {
        return Some(None);
    }
    Some(Some(ActiveHyperlink {
        params: params.to_string(),
        url: url.to_string(),
        terminator,
    }))
}

pub(super) fn format_osc8_hyperlink(link: &ActiveHyperlink) -> String {
    format!(
        "\x1b]8;{};{}{}",
        link.params,
        link.url,
        link.terminator.as_str()
    )
}

pub(super) fn format_osc8_close(terminator: Osc8Terminator) -> String {
    format!("\x1b]8;;{}", terminator.as_str())
}

pub(super) fn get_active_osc8_close(prefix: &str) -> String {
    if !prefix.contains("\x1b]8;") {
        return String::new();
    }
    let mut active: Option<ActiveHyperlink> = None;
    let mut i = 0;
    while i < prefix.len() {
        if let Some((code, len)) = extract_ansi_code(prefix, i) {
            if let Some(link) = parse_osc8_hyperlink(&code) {
                active = link;
            }
            i += len;
        } else {
            let Some(ch) = super::char_at(prefix, i) else {
                break;
            };
            i += ch.len_utf8();
        }
    }
    active
        .map(|l| format_osc8_close(l.terminator))
        .unwrap_or_default()
}
