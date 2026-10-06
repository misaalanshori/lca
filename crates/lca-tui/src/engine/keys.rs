//! Raw terminal input → key identity, ported from pi's
//! `packages/tui/src/keys.ts` (`pi-tui-re/src_re/tui-engine/keys.md`).
//!
//! Three input dialects (Kitty CSI-u, xterm modifyOtherKeys, legacy
//! sequences) normalize into one string `KeyId` vocabulary. pi parses raw
//! bytes; so do we, rather than leaning on `crossterm`'s less complete
//! parser (the brief is explicit about this).
//!
//! Attribution: the dialect tables and the baseLayoutKey policy are pi's
//! (MIT); the raw parse ancestry is sst/opentui's `parse.keypress.ts` (MIT).

mod kitty;
mod legacy;
pub use kitty::EventType;
use kitty::*;
use legacy::*;

use std::sync::atomic::{AtomicBool, Ordering};

/// Global Kitty keyboard protocol state. Set by the terminal after
/// negotiation; consulted by the mode-aware matches below (`\x1b\r` means
/// `shift+enter` under Kitty, `alt+enter` in legacy mode).
static KITTY_PROTOCOL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set the global Kitty keyboard protocol state.
pub fn set_kitty_protocol_active(active: bool) {
    KITTY_PROTOCOL_ACTIVE.store(active, Ordering::SeqCst);
}

/// Query the global Kitty keyboard protocol state.
pub fn is_kitty_protocol_active() -> bool {
    KITTY_PROTOCOL_ACTIVE.load(Ordering::SeqCst)
}

// =============================================================================
// Modifier bits (pi's MODIFIERS + LOCK_MASK)
// =============================================================================

const MOD_SHIFT: u32 = 1;
const MOD_ALT: u32 = 2;
const MOD_CTRL: u32 = 4;
const MOD_SUPER: u32 = 8;
const LOCK_MASK: u32 = 64 + 128; // Caps Lock + Num Lock

const CP_ESCAPE: i64 = 27;
const CP_TAB: i64 = 9;
const CP_ENTER: i64 = 13;
const CP_SPACE: i64 = 32;
const CP_BACKSPACE: i64 = 127;
const CP_KP_ENTER: i64 = 57414;

const ARROW_UP: i64 = -1;
const ARROW_DOWN: i64 = -2;
const ARROW_RIGHT: i64 = -3;
const ARROW_LEFT: i64 = -4;

const FN_DELETE: i64 = -10;
const FN_INSERT: i64 = -11;
const FN_PAGE_UP: i64 = -12;
const FN_PAGE_DOWN: i64 = -13;
const FN_HOME: i64 = -14;
const FN_END: i64 = -15;

/// Kitty keypad codepoints → ASCII/functional equivalents.
fn normalize_kitty_functional_codepoint(cp: i64) -> i64 {
    match cp {
        57399 => 48,
        57400 => 49,
        57401 => 50,
        57402 => 51,
        57403 => 52,
        57404 => 53,
        57405 => 54,
        57406 => 55,
        57407 => 56,
        57408 => 57,
        57409 => 46,
        57410 => 47,
        57411 => 42,
        57412 => 45,
        57413 => 43,
        57415 => 61,
        57416 => 44,
        57417 => ARROW_LEFT,
        57418 => ARROW_RIGHT,
        57419 => ARROW_UP,
        57420 => ARROW_DOWN,
        57421 => FN_PAGE_UP,
        57422 => FN_PAGE_DOWN,
        57423 => FN_HOME,
        57424 => FN_END,
        57425 => FN_INSERT,
        57426 => FN_DELETE,
        other => other,
    }
}

fn normalize_shifted_letter_identity_codepoint(cp: i64, modifier: u32) -> i64 {
    let effective = modifier & !LOCK_MASK;
    if (effective & MOD_SHIFT) != 0 && (65..=90).contains(&cp) {
        return cp + 32;
    }
    cp
}

/// The 30 symbol keys pi treats specially in matching and name formatting.
const SYMBOL_KEYS: &str = "`-=[]\\;',./!@#$%^&*()_+|~{}:<>?";

fn is_known_symbol(cp: i64) -> bool {
    char::from_u32(cp as u32).is_some_and(|c| c.is_ascii() && SYMBOL_KEYS.contains(c))
}

// =============================================================================
// Environment heuristics (transcribed from keys.ts)
// =============================================================================

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

fn is_windows_terminal_session() -> bool {
    env_set("WT_SESSION")
        && !env_set("SSH_CONNECTION")
        && !env_set("SSH_CLIENT")
        && !env_set("SSH_TTY")
}

fn matches_raw_backspace(data: &str, expected_modifier: u32) -> bool {
    if data == "\x7f" {
        return expected_modifier == 0;
    }
    if data != "\x08" {
        return false;
    }
    if is_windows_terminal_session() {
        expected_modifier == MOD_CTRL
    } else {
        expected_modifier == 0
    }
}

// =============================================================================
// rawCtrlChar
// =============================================================================

fn raw_ctrl_char(key: &str) -> Option<char> {
    let ch = key.to_lowercase().chars().next()?;
    let code = ch as u32;
    if (97..=122).contains(&code) || matches!(ch, '[' | '\\' | ']' | '_') {
        return char::from_u32(code & 0x1f);
    }
    if ch == '-' {
        return char::from_u32(31);
    }
    None
}

fn matches_printable_modify_other_keys(
    data: &str,
    expected_keycode: i64,
    expected_modifier: u32,
) -> bool {
    if expected_modifier == 0 {
        return false;
    }
    let Some((code, modifier)) = parse_modify_other_keys(data) else {
        return false;
    };
    if modifier != expected_modifier {
        return false;
    }
    normalize_shifted_letter_identity_codepoint(code, modifier)
        == normalize_shifted_letter_identity_codepoint(expected_keycode, expected_modifier)
}

// =============================================================================
// parseKeyId + matchesKey
// =============================================================================

struct ParsedKeyId {
    key: String,
    modifier: u32,
}

fn parse_key_id(key_id: &str) -> ParsedKeyId {
    let lower = key_id.to_lowercase();
    let parts: Vec<&str> = lower.split('+').collect();
    let key = parts.last().copied().unwrap_or("").to_string();
    let mut modifier = 0;
    if parts.contains(&"shift") {
        modifier |= MOD_SHIFT;
    }
    if parts.contains(&"alt") {
        modifier |= MOD_ALT;
    }
    if parts.contains(&"ctrl") {
        modifier |= MOD_CTRL;
    }
    if parts.contains(&"super") {
        modifier |= MOD_SUPER;
    }
    ParsedKeyId { key, modifier }
}

/// Whether a key identifier names a key `matches_key` can ever match
/// (gh #66 follow-up): the named keys below plus one ASCII
/// letter/digit/symbol, each with any modifier combination. Anything
/// else (a `ctrl+xyz` typo, a bare modifier, an empty string) never
/// matches, so the loader reports it instead of installing silence.
pub fn is_valid_key_id(key_id: &str) -> bool {
    let ParsedKeyId { key, .. } = parse_key_id(key_id);
    match key.as_str() {
        "escape" | "esc" | "space" | "tab" | "enter" | "return" | "backspace" | "insert"
        | "delete" | "clear" | "home" | "end" | "pageup" | "pagedown" | "up" | "down" | "left"
        | "right" | "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10"
        | "f11" | "f12" => true,
        _ => {
            key.chars().count() == 1
                && key.chars().next().is_some_and(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || SYMBOL_KEYS.contains(c)
                })
        }
    }
}

/// Match raw terminal input against a key identifier such as `ctrl+c`,
/// `shift+enter`, or `escape`.
pub fn matches_key(data: &str, key_id: &str) -> bool {
    let ParsedKeyId { key, modifier } = parse_key_id(key_id);
    match key.as_str() {
        "escape" | "esc" => {
            if modifier != 0 {
                return false;
            }
            data == "\x1b"
                || matches_kitty(data, CP_ESCAPE, 0)
                || matches_modify_other_keys(data, CP_ESCAPE, 0)
        }
        "space" => {
            if !is_kitty_protocol_active() {
                if modifier == MOD_CTRL && data == "\x00" {
                    return true;
                }
                if modifier == MOD_ALT && data == "\x1b " {
                    return true;
                }
            }
            if modifier == 0 {
                return data == " "
                    || matches_kitty(data, CP_SPACE, 0)
                    || matches_modify_other_keys(data, CP_SPACE, 0);
            }
            matches_kitty(data, CP_SPACE, modifier)
                || matches_modify_other_keys(data, CP_SPACE, modifier)
        }
        "tab" => {
            if modifier == MOD_SHIFT {
                return data == "\x1b[Z"
                    || matches_kitty(data, CP_TAB, MOD_SHIFT)
                    || matches_modify_other_keys(data, CP_TAB, MOD_SHIFT);
            }
            if modifier == 0 {
                return data == "\t" || matches_kitty(data, CP_TAB, 0);
            }
            matches_kitty(data, CP_TAB, modifier)
                || matches_modify_other_keys(data, CP_TAB, modifier)
        }
        "enter" | "return" => {
            if modifier == MOD_SHIFT {
                if matches_kitty(data, CP_ENTER, MOD_SHIFT)
                    || matches_kitty(data, CP_KP_ENTER, MOD_SHIFT)
                {
                    return true;
                }
                if matches_modify_other_keys(data, CP_ENTER, MOD_SHIFT) {
                    return true;
                }
                if is_kitty_protocol_active() {
                    return data == "\x1b\r" || data == "\n";
                }
                return false;
            }
            if modifier == MOD_ALT {
                if matches_kitty(data, CP_ENTER, MOD_ALT)
                    || matches_kitty(data, CP_KP_ENTER, MOD_ALT)
                {
                    return true;
                }
                if matches_modify_other_keys(data, CP_ENTER, MOD_ALT) {
                    return true;
                }
                if !is_kitty_protocol_active() {
                    return data == "\x1b\r";
                }
                return false;
            }
            if modifier == 0 {
                return data == "\r"
                    || (!is_kitty_protocol_active() && data == "\n")
                    || data == "\x1bOM"
                    || matches_kitty(data, CP_ENTER, 0)
                    || matches_kitty(data, CP_KP_ENTER, 0);
            }
            matches_kitty(data, CP_ENTER, modifier)
                || matches_kitty(data, CP_KP_ENTER, modifier)
                || matches_modify_other_keys(data, CP_ENTER, modifier)
        }
        "backspace" => {
            if modifier == MOD_ALT {
                if data == "\x1b\x7f" || data == "\x1b\x08" {
                    return true;
                }
                return matches_kitty(data, CP_BACKSPACE, MOD_ALT)
                    || matches_modify_other_keys(data, CP_BACKSPACE, MOD_ALT);
            }
            if modifier == MOD_CTRL {
                if matches_raw_backspace(data, MOD_CTRL) {
                    return true;
                }
                return matches_kitty(data, CP_BACKSPACE, MOD_CTRL)
                    || matches_modify_other_keys(data, CP_BACKSPACE, MOD_CTRL);
            }
            if modifier == 0 {
                return matches_raw_backspace(data, 0)
                    || matches_kitty(data, CP_BACKSPACE, 0)
                    || matches_modify_other_keys(data, CP_BACKSPACE, 0);
            }
            matches_kitty(data, CP_BACKSPACE, modifier)
                || matches_modify_other_keys(data, CP_BACKSPACE, modifier)
        }
        "insert" => {
            if modifier == 0 {
                return matches_legacy(legacy_key_sequences("insert"), data)
                    || matches_kitty(data, FN_INSERT, 0);
            }
            matches_legacy_modifier(data, "insert", modifier)
                || matches_kitty(data, FN_INSERT, modifier)
        }
        "delete" => {
            if modifier == 0 {
                return matches_legacy(legacy_key_sequences("delete"), data)
                    || matches_kitty(data, FN_DELETE, 0);
            }
            matches_legacy_modifier(data, "delete", modifier)
                || matches_kitty(data, FN_DELETE, modifier)
        }
        "clear" => {
            if modifier == 0 {
                return matches_legacy(legacy_key_sequences("clear"), data);
            }
            matches_legacy_modifier(data, "clear", modifier)
        }
        "home" => {
            if modifier == 0 {
                return matches_legacy(legacy_key_sequences("home"), data)
                    || matches_kitty(data, FN_HOME, 0);
            }
            matches_legacy_modifier(data, "home", modifier)
                || matches_kitty(data, FN_HOME, modifier)
        }
        "end" => {
            if modifier == 0 {
                return matches_legacy(legacy_key_sequences("end"), data)
                    || matches_kitty(data, FN_END, 0);
            }
            matches_legacy_modifier(data, "end", modifier) || matches_kitty(data, FN_END, modifier)
        }
        "pageup" => {
            if modifier == 0 {
                return matches_legacy(legacy_key_sequences("pageUp"), data)
                    || matches_kitty(data, FN_PAGE_UP, 0);
            }
            matches_legacy_modifier(data, "pageUp", modifier)
                || matches_kitty(data, FN_PAGE_UP, modifier)
        }
        "pagedown" => {
            if modifier == 0 {
                return matches_legacy(legacy_key_sequences("pageDown"), data)
                    || matches_kitty(data, FN_PAGE_DOWN, 0);
            }
            matches_legacy_modifier(data, "pageDown", modifier)
                || matches_kitty(data, FN_PAGE_DOWN, modifier)
        }
        "up" => {
            if modifier == MOD_ALT {
                return data == "\x1bp" || matches_kitty(data, ARROW_UP, MOD_ALT);
            }
            if modifier == 0 {
                return matches_legacy(legacy_key_sequences("up"), data)
                    || matches_kitty(data, ARROW_UP, 0);
            }
            matches_legacy_modifier(data, "up", modifier) || matches_kitty(data, ARROW_UP, modifier)
        }
        "down" => {
            if modifier == MOD_ALT {
                return data == "\x1bn" || matches_kitty(data, ARROW_DOWN, MOD_ALT);
            }
            if modifier == 0 {
                return matches_legacy(legacy_key_sequences("down"), data)
                    || matches_kitty(data, ARROW_DOWN, 0);
            }
            matches_legacy_modifier(data, "down", modifier)
                || matches_kitty(data, ARROW_DOWN, modifier)
        }
        "left" => {
            if modifier == MOD_ALT {
                return data == "\x1b[1;3D"
                    || (!is_kitty_protocol_active() && data == "\x1bB")
                    || data == "\x1bb"
                    || matches_kitty(data, ARROW_LEFT, MOD_ALT);
            }
            if modifier == MOD_CTRL {
                return data == "\x1b[1;5D"
                    || matches_legacy_modifier(data, "left", MOD_CTRL)
                    || matches_kitty(data, ARROW_LEFT, MOD_CTRL);
            }
            if modifier == 0 {
                return matches_legacy(legacy_key_sequences("left"), data)
                    || matches_kitty(data, ARROW_LEFT, 0);
            }
            matches_legacy_modifier(data, "left", modifier)
                || matches_kitty(data, ARROW_LEFT, modifier)
        }
        "right" => {
            if modifier == MOD_ALT {
                return data == "\x1b[1;3C"
                    || (!is_kitty_protocol_active() && data == "\x1bF")
                    || data == "\x1bf"
                    || matches_kitty(data, ARROW_RIGHT, MOD_ALT);
            }
            if modifier == MOD_CTRL {
                return data == "\x1b[1;5C"
                    || matches_legacy_modifier(data, "right", MOD_CTRL)
                    || matches_kitty(data, ARROW_RIGHT, MOD_CTRL);
            }
            if modifier == 0 {
                return matches_legacy(legacy_key_sequences("right"), data)
                    || matches_kitty(data, ARROW_RIGHT, 0);
            }
            matches_legacy_modifier(data, "right", modifier)
                || matches_kitty(data, ARROW_RIGHT, modifier)
        }
        k @ ("f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11"
        | "f12") => {
            if modifier != 0 {
                return false;
            }
            matches_legacy(legacy_key_sequences(k), data)
        }
        _ => {
            // Single letter/digit/symbol keys.
            if key.chars().count() != 1 {
                return false;
            }
            let Some(ch) = key.chars().next() else {
                return false;
            };
            let is_letter = ch.is_ascii_lowercase();
            let is_digit = ch.is_ascii_digit();
            let is_symbol = SYMBOL_KEYS.contains(ch);
            if !(is_letter || is_digit || is_symbol) {
                return false;
            }
            let codepoint = ch as i64;
            let raw_ctrl = raw_ctrl_char(&key);
            if modifier == MOD_CTRL + MOD_ALT
                && !is_kitty_protocol_active()
                && let Some(rc) = raw_ctrl
                && data == format!("\x1b{rc}")
            {
                return true;
            }
            if modifier == MOD_ALT
                && !is_kitty_protocol_active()
                && (is_letter || is_digit || is_symbol)
                && data == format!("\x1b{ch}")
            {
                return true;
            }
            if modifier == MOD_CTRL {
                if let Some(rc) = raw_ctrl
                    && data == rc.to_string()
                {
                    return true;
                }
                return matches_kitty(data, codepoint, MOD_CTRL)
                    || matches_printable_modify_other_keys(data, codepoint, MOD_CTRL);
            }
            if modifier == MOD_SHIFT + MOD_CTRL {
                return matches_kitty(data, codepoint, MOD_SHIFT + MOD_CTRL)
                    || matches_printable_modify_other_keys(data, codepoint, MOD_SHIFT + MOD_CTRL);
            }
            if modifier == MOD_SHIFT {
                if is_letter && data == ch.to_uppercase().to_string() {
                    return true;
                }
                return matches_kitty(data, codepoint, MOD_SHIFT)
                    || matches_printable_modify_other_keys(data, codepoint, MOD_SHIFT);
            }
            if modifier != 0 {
                return matches_kitty(data, codepoint, modifier)
                    || matches_printable_modify_other_keys(data, codepoint, modifier);
            }
            data == key || matches_kitty(data, codepoint, 0)
        }
    }
}

// =============================================================================
// parseKey (display) + printable decoding
// =============================================================================

fn format_key_name_with_modifiers(key_name: &str, modifier: u32) -> Option<String> {
    let effective = modifier & !LOCK_MASK;
    let supported = MOD_SHIFT | MOD_CTRL | MOD_ALT | MOD_SUPER;
    if (effective & !supported) != 0 {
        return None;
    }
    let mut mods: Vec<&str> = Vec::new();
    if effective & MOD_SHIFT != 0 {
        mods.push("shift");
    }
    if effective & MOD_CTRL != 0 {
        mods.push("ctrl");
    }
    if effective & MOD_ALT != 0 {
        mods.push("alt");
    }
    if effective & MOD_SUPER != 0 {
        mods.push("super");
    }
    if mods.is_empty() {
        Some(key_name.to_string())
    } else {
        Some(format!("{}+{}", mods.join("+"), key_name))
    }
}

fn format_parsed_key(
    codepoint: i64,
    modifier: u32,
    base_layout_key: Option<u32>,
) -> Option<String> {
    let normalized = normalize_kitty_functional_codepoint(codepoint);
    let identity = normalize_shifted_letter_identity_codepoint(normalized, modifier);
    let is_latin = (97..=122).contains(&identity);
    let is_digit = (48..=57).contains(&identity);
    let is_symbol = is_known_symbol(identity);
    let effective = if is_latin || is_digit || is_symbol {
        identity
    } else {
        i64::from(base_layout_key.unwrap_or(identity as u32))
    };

    let key_name: String = match effective {
        CP_ESCAPE => "escape".into(),
        CP_TAB => "tab".into(),
        x if x == CP_ENTER || x == CP_KP_ENTER => "enter".into(),
        CP_SPACE => "space".into(),
        CP_BACKSPACE => "backspace".into(),
        x if x == FN_DELETE => "delete".into(),
        x if x == FN_INSERT => "insert".into(),
        x if x == FN_HOME => "home".into(),
        x if x == FN_END => "end".into(),
        x if x == FN_PAGE_UP => "pageUp".into(),
        x if x == FN_PAGE_DOWN => "pageDown".into(),
        x if x == ARROW_UP => "up".into(),
        x if x == ARROW_DOWN => "down".into(),
        x if x == ARROW_LEFT => "left".into(),
        x if x == ARROW_RIGHT => "right".into(),
        x if (48..=57).contains(&x) => char::from_u32(x as u32)?.to_string(),
        x if (97..=122).contains(&x) => char::from_u32(x as u32)?.to_string(),
        x if is_known_symbol(x) => char::from_u32(x as u32)?.to_string(),
        _ => return None,
    };
    format_key_name_with_modifiers(&key_name, modifier)
}

/// Parse raw input and return its key identifier, if recognized (display
/// and logging; the matcher is [`matches_key`]).
pub fn parse_key(data: &str) -> Option<String> {
    if let Some(kitty) = parse_kitty_sequence(data) {
        return format_parsed_key(kitty.codepoint, kitty.modifier, kitty.base_layout_key);
    }
    if let Some((codepoint, modifier)) = parse_modify_other_keys(data) {
        return format_parsed_key(codepoint, modifier, None);
    }
    if is_kitty_protocol_active() && (data == "\x1b\r" || data == "\n") {
        return Some("shift+enter".into());
    }
    if let Some(id) = legacy_sequence_key_id(data) {
        return Some(id.into());
    }
    Some(
        match data {
            "\x1b" => "escape",
            "\x1c" => "ctrl+\\",
            "\x1d" => "ctrl+]",
            "\x1f" => "ctrl+-",
            "\x1b\x1b" => "ctrl+alt+[",
            "\x1b\x1c" => "ctrl+alt+\\",
            "\x1b\x1d" => "ctrl+alt+]",
            "\x1b\x1f" => "ctrl+alt+-",
            "\t" => "tab",
            "\r" => "enter",
            "\x1bOM" => "enter",
            "\x00" => "ctrl+space",
            " " => "space",
            "\x7f" => "backspace",
            "\x08" => {
                return Some(if is_windows_terminal_session() {
                    "ctrl+backspace".into()
                } else {
                    "backspace".into()
                });
            }
            "\x1b[Z" => "shift+tab",
            "\x1b\x7f" | "\x1b\x08" => "alt+backspace",
            _ => {
                if !is_kitty_protocol_active() {
                    match data {
                        "\n" => return Some("enter".into()),
                        "\x1b\r" => return Some("alt+enter".into()),
                        "\x1b " => return Some("alt+space".into()),
                        "\x1bB" => return Some("alt+left".into()),
                        "\x1bF" => return Some("alt+right".into()),
                        _ => {}
                    }
                    let bytes = data.as_bytes();
                    if bytes.len() == 2 && bytes[0] == 0x1b {
                        let code = bytes[1] as u32;
                        if (1..=26).contains(&code) {
                            let c = char::from_u32(code + 96)?;
                            return Some(format!("ctrl+alt+{c}"));
                        }
                        let c = char::from_u32(code)?;
                        if c.is_ascii_lowercase() || c.is_ascii_digit() || SYMBOL_KEYS.contains(c) {
                            return Some(format!("alt+{c}"));
                        }
                    }
                }
                match data {
                    "\x1b[A" => "up",
                    "\x1b[B" => "down",
                    "\x1b[C" => "right",
                    "\x1b[D" => "left",
                    "\x1b[H" | "\x1bOH" => "home",
                    "\x1b[F" | "\x1bOF" => "end",
                    "\x1b[3~" => "delete",
                    "\x1b[5~" => "pageUp",
                    "\x1b[6~" => "pageDown",
                    _ => {
                        if data.chars().count() == 1 {
                            let code = data.chars().next()? as u32;
                            if (1..=26).contains(&code) {
                                let c = char::from_u32(code + 96)?;
                                return Some(format!("ctrl+{c}"));
                            }
                            if (32..=126).contains(&code) {
                                return Some(data.to_string());
                            }
                        }
                        return None;
                    }
                }
            }
        }
        .to_string(),
    )
}

/// Whether the input looks like a Kitty key release (flag 2). Bracketed
/// paste is vetoed: a pasted bluetooth-MAC-shaped `:3F` must not read as a
/// release.
pub fn is_key_release(data: &str) -> bool {
    if data.contains("\x1b[200~") {
        return false;
    }
    [":3u", ":3~", ":3A", ":3B", ":3C", ":3D", ":3H", ":3F"]
        .iter()
        .any(|p| data.contains(p))
}

/// Whether the input looks like a Kitty key repeat (flag 2).
pub fn is_key_repeat(data: &str) -> bool {
    if data.contains("\x1b[200~") {
        return false;
    }
    [":2u", ":2~", ":2A", ":2B", ":2C", ":2D", ":2H", ":2F"]
        .iter()
        .any(|p| data.contains(p))
}

fn decode_kitty_printable(data: &str) -> Option<String> {
    // Reuse the CSI-u parse but keep the shifted key.
    let bytes = data.as_bytes();
    if !bytes.starts_with(b"\x1b[") || bytes.last() != Some(&b'u') {
        return None;
    }
    let body = &bytes[2..bytes.len() - 1];
    let (codepoint, mut i) = digits(body, 0)?;
    let mut shifted: Option<u64> = None;
    if body.get(i) == Some(&b':') {
        i += 1;
        let start = i;
        while i < body.len() && body[i].is_ascii_digit() {
            i += 1;
        }
        if i > start {
            shifted = std::str::from_utf8(&body[start..i]).ok()?.parse().ok();
        }
        if body.get(i) == Some(&b':') {
            i += 1;
            if let Some((_, ni)) = digits(body, i) {
                i = ni;
            }
        }
    }
    let mut mod_value = 1u64;
    if body.get(i) == Some(&b';') {
        i += 1;
        let (m, ni) = digits(body, i)?;
        mod_value = m;
        i = ni;
        if body.get(i) == Some(&b':') {
            i += 1;
            if let Some((_, ni)) = digits(body, i) {
                i = ni;
            }
        }
    }
    if i != body.len() {
        return None;
    }
    let modifier = mod_value.saturating_sub(1) as u32;
    let allowed = MOD_SHIFT | LOCK_MASK;
    if (modifier & !allowed) != 0 || (modifier & (MOD_ALT | MOD_CTRL)) != 0 {
        return None;
    }
    let mut effective = codepoint;
    if (modifier & MOD_SHIFT) != 0
        && let Some(s) = shifted
    {
        effective = s;
    }
    let effective = normalize_kitty_functional_codepoint(effective as i64);
    if effective < 32 {
        return None;
    }
    char::from_u32(effective as u32).map(|c| c.to_string())
}

fn decode_modify_other_keys_printable(data: &str) -> Option<String> {
    let (codepoint, modifier) = parse_modify_other_keys(data)?;
    let modifier = modifier & !LOCK_MASK;
    if (modifier & !MOD_SHIFT) != 0 || codepoint < 32 {
        return None;
    }
    char::from_u32(codepoint as u32).map(|c| c.to_string())
}

/// Decode raw input into the printable text it should insert, if any.
pub fn decode_printable_key(data: &str) -> Option<String> {
    decode_kitty_printable(data).or_else(|| decode_modify_other_keys_printable(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_kitty<T>(active: bool, f: T)
    where
        T: FnOnce(),
    {
        let prev = is_kitty_protocol_active();
        set_kitty_protocol_active(active);
        f();
        set_kitty_protocol_active(prev);
    }

    // Verifies: gh #66 follow-up - the key vocabulary accepts every
    // shape `matches_key` handles and rejects anything else, so a
    // `ctrl+xyz` typo reports loud instead of never matching.
    #[test]
    fn the_key_vocabulary_accepts_named_and_single_keys_only() {
        for valid in [
            "up",
            "down",
            "left",
            "right",
            "pageUp",
            "pageDown",
            "home",
            "end",
            "escape",
            "esc",
            "enter",
            "return",
            "tab",
            "space",
            "backspace",
            "delete",
            "insert",
            "clear",
            "f1",
            "f12",
            "a",
            "z",
            "5",
            ";",
            "ctrl+x",
            "ctrl+shift+n",
            "alt+left",
            "super+k",
        ] {
            assert!(is_valid_key_id(valid), "{valid} is a key");
        }
        for invalid in ["", "ctrl+xyz", "super-duper", "shift", "f13", "enter+"] {
            assert!(!is_valid_key_id(invalid), "{invalid} is not a key");
        }
    }

    // Verifies: I.1 key dialect tables (pi keys.ts golden cases).
    #[test]
    fn legacy_sequences_match_their_key_ids() {
        assert!(matches_key("\x1b[A", "up"));
        assert!(matches_key("\x1bOA", "up"));
        assert!(matches_key("\x1b[B", "down"));
        assert!(matches_key("\x1b[5~", "pageUp"));
        assert!(matches_key("\x1b[6~", "pageDown"));
        assert!(matches_key("\x1b[H", "home"));
        assert!(matches_key("\x1b[F", "end"));
        assert!(matches_key("\x1b[3~", "delete"));
        assert!(matches_key("\x1b[2~", "insert"));
        assert!(matches_key("\x1bOP", "f1"));
        assert!(matches_key("\x1b[24~", "f12"));
    }

    #[test]
    fn ctrl_letters_use_control_codes() {
        assert!(matches_key("\x03", "ctrl+c"));
        assert!(matches_key("\x1b[99;5u", "ctrl+c")); // Kitty CSI-u
        assert!(matches_key("\x1b[27;5;99~", "ctrl+c")); // modifyOtherKeys
        assert!(!matches_key("\x03", "ctrl+d"));
    }

    #[test]
    fn shift_tab_and_shift_enter_dialects() {
        assert!(matches_key("\x1b[Z", "shift+tab"));
        assert!(matches_key("\x1b[9;2u", "shift+tab"));
        assert!(matches_key("\x1b[13;2u", "shift+enter"));
        with_kitty(true, || {
            assert!(matches_key("\x1b\r", "shift+enter"));
            assert!(matches_key("\n", "shift+enter"));
            assert!(!matches_key("\x1b\r", "alt+enter"));
        });
        with_kitty(false, || {
            assert!(matches_key("\x1b\r", "alt+enter"));
        });
    }

    #[test]
    fn base_layout_key_policy_prefers_recognizable_codepoints() {
        // Latin recognisable -> codepoint authoritative, base layout ignored.
        assert!(!matches_key("\x1b[118:118:107;5u", "ctrl+k")); // reports v with base k
        // Cyrillic -> base layout key rescues the match.
        assert!(matches_key("\x1b[1089:0:99;5u", "ctrl+c"));
    }

    #[test]
    fn keypad_normalizes_to_ascii_equivalents() {
        assert!(matches_key("\x1b[57404u", "5"));
        assert!(matches_key("\x1b[57414u", "enter"));
    }

    #[test]
    fn escape_alone_and_alt_prefixed_printables() {
        assert!(matches_key("\x1b", "escape"));
        assert!(matches_key("\x1b[27u", "escape"));
        assert!(matches_key("\x1b[27;1u", "escape"));
        assert!(matches_key("\x1b[27;1;27~", "escape"));
        with_kitty(false, || {
            assert!(matches_key("\x1bx", "alt+x"));
            assert!(matches_key("\x1b\x03", "ctrl+alt+c"));
        });
    }

    #[test]
    fn arrow_modifier_dialects() {
        assert!(matches_key("\x1b[1;3D", "alt+left"));
        assert!(matches_key("\x1b[1;5C", "ctrl+right"));
        assert!(matches_key("\x1bb", "alt+left"));
        assert!(matches_key("\x1bf", "alt+right"));
    }

    #[test]
    fn release_and_repeat_sniffing_vetoes_paste() {
        assert!(is_key_release("\x1b[99;5:3u"));
        assert!(is_key_repeat("\x1b[99;5:2u"));
        assert!(!is_key_release("\x1b[200~90:62:3F:A5\x1b[201~"));
        assert!(!is_key_repeat("\x1b[200~x:2F:y\x1b[201~"));
    }

    #[test]
    fn parse_key_round_trips_common_inputs() {
        assert_eq!(parse_key("\x1b[A").as_deref(), Some("up"));
        assert_eq!(parse_key("\r").as_deref(), Some("enter"));
        assert_eq!(parse_key("\x03").as_deref(), Some("ctrl+c"));
        assert_eq!(parse_key("\x1b[Z").as_deref(), Some("shift+tab"));
        assert_eq!(parse_key("a").as_deref(), Some("a"));
        assert_eq!(parse_key("\x1b[13;2u").as_deref(), Some("shift+enter"));
    }

    #[test]
    fn decode_printable_accepts_plain_and_shifted_only() {
        assert_eq!(decode_printable_key("\x1b[97u").as_deref(), Some("a"));
        assert_eq!(decode_printable_key("\x1b[97:65;2u").as_deref(), Some("A"));
        assert_eq!(decode_printable_key("\x1b[27;2;97~").as_deref(), Some("a"));
        assert!(decode_printable_key("\x1b[97;5u").is_none()); // ctrl rejected
        assert!(decode_printable_key("\x1b[97;3u").is_none()); // alt rejected
    }
}
