//! Kitty CSI-u and modifyOtherKeys parsing (split from `keys.rs`, P5).

use super::*;

// =============================================================================
// Kitty CSI-u parsing (hand-parsed; no regex dependency)
// =============================================================================

/// Key event type from Kitty flag 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    /// Press.
    Press,
    /// Repeat.
    Repeat,
    /// Release.
    Release,
}

impl EventType {
    fn from_str(s: Option<&str>) -> Self {
        match s.and_then(|v| v.parse::<u32>().ok()) {
            Some(2) => EventType::Repeat,
            Some(3) => EventType::Release,
            _ => EventType::Press,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ParsedKitty {
    pub(super) codepoint: i64,
    pub(super) base_layout_key: Option<u32>,
    pub(super) modifier: u32,
    #[allow(dead_code)] // retained for parity with pi; release/repeat use substring sniffers
    event_type: EventType,
}

/// Parse a non-negative integer run at `bytes[i..]`, returning `(value, next)`.
pub(super) fn digits(bytes: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let start = i;
    let mut value: u64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        value = value
            .checked_mul(10)?
            .checked_add((bytes[i] - b'0') as u64)?;
        i += 1;
    }
    if i == start { None } else { Some((value, i)) }
}

pub(super) fn parse_kitty_sequence(data: &str) -> Option<ParsedKitty> {
    let bytes = data.as_bytes();
    if !bytes.starts_with(b"\x1b[") {
        return None;
    }
    // CSI-u: ESC [ cp ( : shifted? )? ( : base )? ( ; mod ( : event )? )? u
    if bytes.last() == Some(&b'u') {
        let body = &bytes[2..bytes.len() - 1];
        let (cp, mut i) = digits(body, 0)?;
        let mut base_layout: Option<u64> = None;
        // shifted key: `:digits?` (possibly empty)
        if body.get(i) == Some(&b':') {
            i += 1;
            // optional digits for shifted key; skip
            while i < body.len() && body[i].is_ascii_digit() {
                i += 1;
            }
            // base layout key: another `:digits`
            if body.get(i) == Some(&b':') {
                i += 1;
                if let Some((b, ni)) = digits(body, i) {
                    base_layout = Some(b);
                    i = ni;
                }
            }
        }
        let mut mod_value = 1u64;
        let mut event: Option<&str> = None;
        if body.get(i) == Some(&b';') {
            i += 1;
            let (m, ni) = digits(body, i)?;
            mod_value = m;
            i = ni;
            if body.get(i) == Some(&b':') {
                i += 1;
                let (e, ni) = digits(body, i)?;
                let _ = e;
                // Re-slice the numeric run as a &str for EventType::from_str.
                let start = i;
                i = ni;
                event = Some(std::str::from_utf8(&body[start..ni]).ok()?);
            }
        }
        if i != body.len() {
            return None;
        }
        return Some(ParsedKitty {
            codepoint: cp as i64,
            base_layout_key: base_layout.map(|v| v as u32),
            modifier: u32::try_from(mod_value.saturating_sub(1)).unwrap_or(u32::MAX),
            event_type: EventType::from_str(event),
        });
    }

    // Arrow: ESC [ 1 ; mod ( : event )? [ABCD]
    if matches!(bytes.last(), Some(b'A' | b'B' | b'C' | b'D')) && bytes.starts_with(b"\x1b[1;") {
        let final_byte = *bytes.last()?;
        let body = &bytes[2..bytes.len() - 1];
        // body == "1;mod" or "1;mod:event"
        let semi = body.iter().position(|&b| b == b';')?;
        let (mod_value, mut i) = digits(body, semi + 1)?;
        let mut event = None;
        if body.get(i) == Some(&b':') {
            i += 1;
            let start = i;
            let (_, ni) = digits(body, i)?;
            i = ni;
            event = Some(std::str::from_utf8(&body[start..ni]).ok()?);
        }
        if i != body.len() {
            return None;
        }
        let cp = match final_byte {
            b'A' => ARROW_UP,
            b'B' => ARROW_DOWN,
            b'C' => ARROW_RIGHT,
            _ => ARROW_LEFT,
        };
        return Some(ParsedKitty {
            codepoint: cp,
            base_layout_key: None,
            modifier: u32::try_from(mod_value.saturating_sub(1)).unwrap_or(u32::MAX),
            event_type: EventType::from_str(event),
        });
    }

    // Functional: ESC [ num ( ; mod )? ( : event )? ~
    if bytes.last() == Some(&b'~') && bytes.starts_with(b"\x1b[") {
        let body = &bytes[2..bytes.len() - 1];
        let (key_num, mut i) = digits(body, 0)?;
        let mut mod_value = 1u64;
        let mut event = None;
        if body.get(i) == Some(&b';') {
            i += 1;
            let (m, ni) = digits(body, i)?;
            mod_value = m;
            i = ni;
        }
        if body.get(i) == Some(&b':') {
            i += 1;
            let start = i;
            let (_, ni) = digits(body, i)?;
            i = ni;
            event = Some(std::str::from_utf8(&body[start..ni]).ok()?);
        }
        if i != body.len() {
            return None;
        }
        let cp = match key_num {
            2 => FN_INSERT,
            3 => FN_DELETE,
            5 => FN_PAGE_UP,
            6 => FN_PAGE_DOWN,
            7 => FN_HOME,
            8 => FN_END,
            _ => return None,
        };
        return Some(ParsedKitty {
            codepoint: cp,
            base_layout_key: None,
            modifier: u32::try_from(mod_value.saturating_sub(1)).unwrap_or(u32::MAX),
            event_type: EventType::from_str(event),
        });
    }

    // Home/End: ESC [ 1 ; mod ( : event )? [HF]
    if matches!(bytes.last(), Some(b'H' | b'F')) && bytes.starts_with(b"\x1b[1;") {
        let final_byte = *bytes.last()?;
        let body = &bytes[2..bytes.len() - 1];
        let semi = body.iter().position(|&b| b == b';')?;
        let (mod_value, mut i) = digits(body, semi + 1)?;
        let mut event = None;
        if body.get(i) == Some(&b':') {
            i += 1;
            let start = i;
            let (_, ni) = digits(body, i)?;
            i = ni;
            event = Some(std::str::from_utf8(&body[start..ni]).ok()?);
        }
        if i != body.len() {
            return None;
        }
        return Some(ParsedKitty {
            codepoint: if final_byte == b'H' { FN_HOME } else { FN_END },
            base_layout_key: None,
            modifier: u32::try_from(mod_value.saturating_sub(1)).unwrap_or(u32::MAX),
            event_type: EventType::from_str(event),
        });
    }

    None
}

pub(super) fn matches_kitty(data: &str, expected_codepoint: i64, expected_modifier: u32) -> bool {
    let Some(parsed) = parse_kitty_sequence(data) else {
        return false;
    };
    let actual_mod = parsed.modifier & !LOCK_MASK;
    let expected_mod = expected_modifier & !LOCK_MASK;
    if actual_mod != expected_mod {
        return false;
    }
    let normalized = normalize_shifted_letter_identity_codepoint(
        normalize_kitty_functional_codepoint(parsed.codepoint),
        parsed.modifier,
    );
    let normalized_expected = normalize_shifted_letter_identity_codepoint(
        normalize_kitty_functional_codepoint(expected_codepoint),
        expected_modifier,
    );
    if normalized == normalized_expected {
        return true;
    }
    // Alternate match: base layout key, only when the codepoint is not a
    // recognized Latin letter or symbol (the remapped-layouts policy).
    if let Some(base) = parsed.base_layout_key
        && i64::from(base) == expected_codepoint
    {
        let is_latin = (97..=122).contains(&normalized);
        if !is_latin && !is_known_symbol(normalized) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// modifyOtherKeys: ESC [ 27 ; mod ; code ~
// ---------------------------------------------------------------------------

pub(super) fn parse_modify_other_keys(data: &str) -> Option<(i64, u32)> {
    let bytes = data.as_bytes();
    if !bytes.starts_with(b"\x1b[27;") || bytes.last() != Some(&b'~') {
        return None;
    }
    let body = &bytes[2..bytes.len() - 1];
    let mut i = 0;
    let (_, ni) = digits(body, i)?; // the literal 27
    i = ni;
    if body.get(i) != Some(&b';') {
        return None;
    }
    i += 1;
    let (mod_value, ni) = digits(body, i)?;
    i = ni;
    if body.get(i) != Some(&b';') {
        return None;
    }
    i += 1;
    let (code, ni) = digits(body, i)?;
    i = ni;
    if i != body.len() {
        return None;
    }
    Some((
        i64::try_from(code).unwrap_or(i64::MAX),
        u32::try_from(mod_value.saturating_sub(1)).unwrap_or(u32::MAX),
    ))
}

pub(super) fn matches_modify_other_keys(
    data: &str,
    expected_keycode: i64,
    expected_modifier: u32,
) -> bool {
    parse_modify_other_keys(data)
        .is_some_and(|(code, modifier)| code == expected_keycode && modifier == expected_modifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_parse_a_run_and_stop() {
        assert_eq!(digits(b"123;45", 0), Some((123, 3)));
        assert_eq!(digits(b";", 0), None);
        assert_eq!(digits(b"", 0), None);
    }
}
