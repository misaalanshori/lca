//! The shared bracketed-paste primitive (TUI cycle 7, R1/R6).
//!
//! Before this module, paste was the editor's private trick: the editor
//! caught `ESC[200~…ESC[201~` in `handle_key`, and every other text
//! surface (the masked secret field, the base-URL and model-id fields,
//! the picker search boxes) saw only keys and silently dropped paste.
//! pi's answer is that paste is a primitive of *every* text input — its
//! `components/input.ts` carries its own paste buffer beside the editor's
//! — and this is that primitive in one place.
//!
//! Two normalization contracts share the decode step:
//!
//! - [`normalize`] is the multi-line editor's: decode tmux's CSI-u
//!   re-encoding of control bytes, normalize line endings, expand tabs,
//!   keep newlines, drop other non-printables.
//! - [`flatten`] is the single-line contract pi's `input.ts` states:
//!   the same text with newlines removed, so a multi-line paste cannot
//!   break a one-line field.
//!
//! Markers (`[paste #1 …]`) are the editor's alone; a single-line field
//! inserts the flattened text verbatim.

/// The bracketed-paste start marker (xterm/DECSET 2004).
pub const PASTE_START: &str = "\x1b[200~";
/// The bracketed-paste end marker.
pub const PASTE_END: &str = "\x1b[201~";

/// The content of one complete bracketed-paste event, if `data` is one.
///
/// The engine reassembles a paste into a single `Data` event and the
/// terminal layer re-wraps it with the markers, so every consumer sees
/// one uniform shape.
pub fn bracketed_paste_content(data: &str) -> Option<&str> {
    data.strip_prefix(PASTE_START)?.strip_suffix(PASTE_END)
}

/// Decode tmux's `extended-keys=csi-u` re-encoding of control bytes inside
/// a bracketed paste (`ESC [ <cp> ; 5 u` -> the literal control byte).
///
/// Without this a pasted newline arrives as `ESC[106;5u`, the filter
/// below strips the ESC, and the printable tail (`[106;5u`) leaks into
/// the field as text (pi's `handlePaste` comment says the same).
pub fn decode_csi_u_ctrl(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            let mut j = i + 2;
            let start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start
                && bytes.get(j) == Some(&b';')
                && bytes.get(j + 1) == Some(&b'5')
                && bytes.get(j + 2) == Some(&b'u')
            {
                let cp: u32 = text[start..j].parse().unwrap_or(0);
                let decoded = if (97..=122).contains(&cp) {
                    char::from_u32(cp - 96)
                } else if (65..=90).contains(&cp) {
                    char::from_u32(cp - 64)
                } else {
                    None
                };
                if let Some(c) = decoded {
                    out.push(c);
                    i = j + 3;
                    continue;
                }
            }
        }
        match text[i..].chars().next() {
            Some(c) => {
                out.push(c);
                i += c.len_utf8();
            }
            None => break,
        }
    }
    out
}

/// The editor's paste normalization: decode tmux CSI-u, CRLF/CR -> LF,
/// tabs -> 4 spaces, keep newlines, drop every other control character.
pub fn normalize(content: &str) -> String {
    let decoded = decode_csi_u_ctrl(content);
    let normalized = decoded
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ");
    normalized
        .chars()
        .filter(|c| *c == '\n' || (*c as u32) >= 32)
        .collect()
}

/// The single-line field's paste normalization: [`normalize`] with the
/// newlines removed (pi's `input.ts` "remove newlines and carriage
/// returns" contract). Tabs still expand, CSI-u still decodes.
pub fn flatten(content: &str) -> String {
    normalize(content).replace('\n', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: R1/R6 - a real bracketed paste is recognized and its
    // content extracted without the markers.
    #[test]
    fn paste_extracts_the_content_between_the_markers() {
        assert_eq!(
            bracketed_paste_content("\x1b[200~hello\x1b[201~"),
            Some("hello")
        );
        assert_eq!(bracketed_paste_content("plain"), None);
        assert_eq!(bracketed_paste_content("\x1b[200~unterminated"), None);
    }

    // Verifies: R1 - the editor contract expands tabs and normalizes
    // line endings while keeping newlines as line breaks.
    #[test]
    fn paste_normalizes_crlf_and_tabs_and_keeps_newlines() {
        assert_eq!(normalize("a\r\nb\rc\td"), "a\nb\nc    d");
    }

    // Verifies: R1 - tmux's CSI-u re-encoding of a pasted newline decodes
    // to a real newline instead of leaking `[106;5u` text.
    #[test]
    fn paste_decodes_tmux_csi_u_ctrl_bytes() {
        // 106 -> 'j' -> 10 -> newline; 65 -> 'A' -> 1 -> SOH (dropped).
        assert_eq!(normalize("a\x1b[106;5ub"), "a\nb");
        assert_eq!(normalize("x\x1b[65;5uy"), "xy");
    }

    // Verifies: R1 - the single-line contract removes newlines so a
    // multi-line paste cannot break a one-line field.
    #[test]
    fn paste_flattens_newlines_for_single_line_fields() {
        assert_eq!(flatten("a\nb\r\nc"), "abc");
        assert_eq!(flatten("one\ttwo"), "one    two");
        assert_eq!(flatten("a\x1b[106;5ub"), "ab");
    }

    // Verifies: R1 - non-printable control bytes are dropped, never left
    // to reach a frame.
    #[test]
    fn paste_drops_control_characters() {
        assert_eq!(normalize("a\x07b\x1b[31mc"), "ab[31mc");
        assert_eq!(flatten("a\x00b"), "ab");
    }

    // Verifies: R1 - a paste is byte-for-byte stable for ordinary text.
    #[test]
    fn paste_leaves_ordinary_text_alone() {
        let text = "https://api.example.com/v1?x=1&y=2";
        assert_eq!(normalize(text), text);
        assert_eq!(flatten(text), text);
    }
}
