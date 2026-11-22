//! A tool card's one-line argument summary: a small **grammar** per tool rather than one
//! `path` lookup and a raw-JSON fallback (gh #392 slice C).
//!
//! # What was wrong
//!
//! `format_tool_args` matched five names and otherwise printed the whole argument string:
//!
//! * a `read` card said `src/lib.rs` even when the call asked for lines 180–239, so a reader
//!   could not tell a one-line read from a whole-file one;
//! * an unknown tool's card carried 400 characters of JSON on a *collapsed* row;
//! * a non-string argument vanished (the `path` extraction answered `None` and the raw
//!   string took over), and an empty streaming `"path": ""` rendered as nothing at all,
//!   which reads as a card that failed to paint rather than one still filling in.
//!
//! # The grammar
//!
//! | tool | summary |
//! |---|---|
//! | `read` | `path:start-end` (`:start` when only `offset` is given), `~`-shortened, OSC 8 linked |
//! | `write`, `edit`, `list`, `ls`, `glob` | `path` (`~`-shortened + linked) |
//! | `grep` | `/pattern/` |
//! | `bash`, `shell` | the command, plus a muted-shape `(timeout Ns)` when the call sets one |
//! | anything else | `name key="value" …`, capped at 100 characters |
//!
//! # The defensive numeric coercion, and the trap it exists for
//!
//! `offset`/`limit` may arrive as JSON numbers *or* as numeric strings, because models emit
//! both. The failure this guards against is arithmetic on the raw text: `"180"` and `"60"`
//! concatenated into `:18060` (the issue's own example). So both forms are parsed to
//! `usize` first, and anything that is not a finite positive number is treated as
//! **absent** — a full-file read shows no range at all, never `:1`, and never a range built
//! from a `null`, a missing field, a negative count or a non-numeric string.

use crate::state::display_path;

/// How long a collapsed unknown-tool summary may be, in visible characters.
///
/// pi's own cap for the same row. A JSON blob on a collapsed card is the worst of both
/// worlds - too long to scan, too short to be the payload - so the card shows a bounded
/// prefix and the expanded card shows the fields.
pub(crate) const UNKNOWN_SUMMARY_CAP: usize = 100;

/// A tool call's argument value, as far as a one-line summary needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    /// A JSON string, already unescaped.
    Text(String),
    /// Anything else (number, bool, null, nested object): its raw token text.
    Raw(String),
}

impl Value {
    /// The value as a number, when it is one - a JSON number or a numeric string.
    ///
    /// This is where the `"180"`/`"60"` trap is closed: both forms become a `usize` before
    /// any arithmetic, so no code path can concatenate them.
    fn number(&self) -> Option<usize> {
        let text = match self {
            Value::Text(text) => text.trim(),
            Value::Raw(text) => text.trim(),
        };
        text.parse::<usize>().ok().filter(|n| *n > 0)
    }

    /// Whether this argument is a value the summary can print at all.
    ///
    /// `null` is the streaming shape - a field the model has announced but not filled - so
    /// it is reported as *absent* rather than as the string `null`.
    fn is_absent(&self) -> bool {
        matches!(self, Value::Raw(raw) if raw == "null")
    }
}

/// Parse a flat JSON object into ordered key/value pairs.
///
/// Hand-written rather than `serde_json` because this is display code on the render path:
/// it needs order (a summary reads in the model's order), it must survive a **partial**
/// object (streaming arguments are a prefix of one), and it must never fail loudly on a
/// blob it does not understand. A value that is not a string is kept as raw text - the
/// caller decides what that means, and for `offset`/`limit` it may be a number.
fn flat_object(args: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let bytes: Vec<char> = args.chars().collect();
    let mut index = 0usize;
    /// Skip whitespace.
    fn skip_space(chars: &[char], at: &mut usize) {
        while *at < chars.len() && chars[*at].is_whitespace() {
            *at += 1;
        }
    }
    skip_space(&bytes, &mut index);
    if bytes.get(index) != Some(&'{') {
        return out;
    }
    index += 1;
    loop {
        skip_space(&bytes, &mut index);
        match bytes.get(index) {
            Some('"') => index += 1,
            // End of object, or a truncated prefix: whatever has been read is the answer.
            _ => return out,
        }
        // The key, with escapes decoded the same way a string value is.
        let mut key = String::new();
        while let Some(&ch) = bytes.get(index) {
            index += 1;
            match ch {
                '"' => break,
                '\\' => {
                    let (decoded, consumed) = decode_escape(&bytes[index..]);
                    key.push(decoded);
                    index += consumed;
                }
                ch => key.push(ch),
            }
        }
        skip_space(&bytes, &mut index);
        if bytes.get(index) != Some(&':') {
            return out;
        }
        index += 1;
        skip_space(&bytes, &mut index);
        let value = if bytes.get(index) == Some(&'"') {
            index += 1;
            let mut text = String::new();
            while let Some(&ch) = bytes.get(index) {
                index += 1;
                match ch {
                    '"' => break,
                    '\\' => {
                        let (decoded, consumed) = decode_escape(&bytes[index..]);
                        text.push(decoded);
                        index += consumed;
                    }
                    ch => text.push(ch),
                }
            }
            Value::Text(text)
        } else {
            // A bare token - number, bool, null, or a nested array/object kept verbatim as
            // far as its brackets balance. Depth tracking is what lets an argument summary
            // survive `{"filter":{"a":1}}` without printing a fragment of it as the value.
            let start = index;
            let mut depth = 0i32;
            let mut in_string = false;
            let mut escaped = false;
            while let Some(&ch) = bytes.get(index) {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        in_string = false;
                    }
                    index += 1;
                    continue;
                }
                match ch {
                    '"' => in_string = true,
                    '{' | '[' => depth += 1,
                    '}' | ']' if depth == 0 => break,
                    '}' | ']' => depth -= 1,
                    ',' if depth == 0 => break,
                    _ => {}
                }
                index += 1;
            }
            Value::Raw(
                bytes[start..index]
                    .iter()
                    .collect::<String>()
                    .trim()
                    .to_string(),
            )
        };
        out.push((key, value));
        skip_space(&bytes, &mut index);
        if bytes.get(index) == Some(&',') {
            index += 1;
            continue;
        }
        return out;
    }
}

/// Decode one JSON escape body (the backslash is already consumed).
///
/// Returns the character and how many input characters it consumed; `\uXXXX` defers to
/// [`decode_unicode_escape`], which is the decoder gh #405 had to fix.
fn decode_escape(chars: &[char]) -> (char, usize) {
    match chars.first() {
        Some('n') => ('\n', 1),
        Some('t') => ('\t', 1),
        Some('r') => ('\r', 1),
        Some('b') => ('\u{8}', 1),
        Some('f') => ('\u{c}', 1),
        Some('u') => decode_unicode_escape(chars),
        Some(other) => (*other, 1),
        None => ('\u{FFFD}', 0),
    }
}

/// Decode one `\uXXXX` escape body (the `u` is already consumed) into its character and how
/// many input characters it used.
///
/// Gh #405 gap 4's decoder, moved here with the grammar it belongs to (gh #392 slice C) and
/// **kept faithful**: this used to skip the four hex digits and push `U+FFFD`, so every
/// non-ASCII character in a tool argument became a replacement character in the card's
/// headline. JSON escapes every non-ASCII character by default, so that was not an edge case.
///
/// Surrogate pairs are decoded properly (`\ud83c\udf89` → 🎉), because half a pair is not a
/// character and emitting one is how mojibake starts. A pair that is not well formed yields
/// `U+FFFD` once for the pair, which is the honest answer for input that cannot be text.
fn decode_unicode_escape(chars: &[char]) -> (char, usize) {
    /// Four hex digits, ASCII only, starting at `offset`.
    fn four_hex(chars: &[char], offset: usize) -> Option<u32> {
        let mut value = 0u32;
        for step in 0..4 {
            let digit = chars.get(offset + step)?.to_digit(16)?;
            value = value.checked_mul(16)? + digit;
        }
        Some(value)
    }
    let Some(first) = four_hex(chars, 1) else {
        // A truncated escape: one replacement for the `u`, and the input after it untouched.
        return ('\u{FFFD}', 1);
    };
    // The lead `u` plus its four digits.
    const ESCAPE: usize = 5;
    if (0xD800..0xDC00).contains(&first) {
        // A high surrogate must be followed by `\u` and a low surrogate. The lookahead reads
        // without consuming, so a byte that is not the start of a pair stays in the input:
        // consuming it and finding "not a backslash" would silently eat the character after a
        // malformed escape (measured on the original row: `"a\ud83cb"` lost the `b`).
        let pair = || {
            if *chars.get(ESCAPE)? != '\\' || *chars.get(ESCAPE + 1)? != 'u' {
                return None;
            }
            four_hex(chars, ESCAPE + 2)
        };
        return match pair() {
            Some(low) if (0xDC00..0xE000).contains(&low) => (
                // Both halves are surrogates, so the combined scalar is in range.
                char::from_u32(0x1_0000 + ((first - 0xD800) << 10) + (low - 0xDC00))
                    .unwrap_or('\u{FFFD}'),
                ESCAPE * 2 + 1,
            ),
            // A lone or mismatched high surrogate: replaced once, and the input after it is
            // untouched.
            _ => ('\u{FFFD}', ESCAPE),
        };
    }
    // `char::from_u32` already refuses a lone surrogate (and anything above `0x10FFFF`) by
    // answering `None`, so that case is the replacement character and nothing else.
    (char::from_u32(first).unwrap_or('\u{FFFD}'), ESCAPE)
}

/// A JSON string field's value, unescaped (`None` when absent, `null`, or not a string).
fn text_field(args: &str, key: &str) -> Option<String> {
    flat_object(args)
        .into_iter()
        .find(|(name, _)| name == key)
        .and_then(|(_, value)| match value {
            Value::Text(text) if !text.is_empty() => Some(text),
            _ => None,
        })
}

/// `path:start-end` for a `read` call, or `None` when the call asked for the whole file.
///
/// The three shapes the issue names, and the defensive rules beside them:
///
/// * `offset` + `limit` → `offset-(offset+limit-1)`, so `100`/`50` reads `100-149`;
/// * `offset` alone → `offset`, a lower-bounded read;
/// * `limit` alone, `null`, missing, zero, negative, non-numeric → **no range**, because a
///   range built from any of those would name lines the call never asked for.
fn line_range(offset: Option<&Value>, limit: Option<&Value>) -> Option<String> {
    let offset = offset.and_then(Value::number)?;
    match limit.and_then(Value::number) {
        Some(limit) => Some(format!("{offset}-{}", offset + limit - 1)),
        // Only `offset`: the call asked from a line to the end.
        None => Some(format!("{offset}")),
    }
}

/// Shorten a path for display and link it when the terminal forwards hyperlinks.
///
/// Both halves are the house rules: `display_path` strips Windows' `\\?\` prefix (which is
/// load-bearing internally and noise in a card), `shorten_home` gives `~/…`, and the OSC 8
/// wrapper is pi's - emitted only when [`lca_tui::engine::terminal::supports_hyperlinks`]
/// says the terminal keeps it, because a swallowed link leaves a reader with a path they
/// cannot click *and* cannot read.
fn render_path(path: &str) -> String {
    let shown = crate::footer::shorten_home(&display_path(path), None);
    if !lca_tui::engine::terminal::supports_hyperlinks() {
        return shown;
    }
    let url = format!("file://{}", absolute_hint(path));
    format!("\x1b]8;;{url}\x1b\\{shown}\x1b]8;;\x1b\\")
}

/// The absolute path an OSC 8 `file://` link should carry.
///
/// A relative argument (`src/lib.rs`) is resolved against the process's working directory,
/// because `file://src/lib.rs` is not a URL any terminal can open. When the path cannot be
/// made absolute the argument is used as-is: a link that may not resolve is better than a
/// link to the wrong file.
fn absolute_hint(path: &str) -> String {
    let path = std::path::Path::new(path);
    if path.is_absolute() {
        return display_path(&path.display().to_string());
    }
    std::env::current_dir()
        .map(|dir| display_path(&dir.join(path).display().to_string()))
        .unwrap_or_else(|_| path.display().to_string())
}

/// `name key="value" …`, capped at [`UNKNOWN_SUMMARY_CAP`] characters.
///
/// The cap counts **characters**, not bytes, so a CJK argument is not cut mid-sequence into
/// mojibake - the same reason `gh #405` had to decode `\uXXXX` properly. The ellipsis is
/// appended inside the cap rather than after it, so the row is never one column over.
fn unknown_summary(args: &str) -> String {
    let fields = flat_object(args);
    // Not a JSON object at all (a streaming fragment, or a producer that sent prose): the
    // text it did send is the only honest summary, and the cap still applies. This is also
    // what a purpose-built tool falls back to when its arguments are not an object.
    let mut out = if fields.is_empty() {
        args.trim().to_string()
    } else {
        String::new()
    };
    for (key, value) in &fields {
        if value.is_absent() {
            continue;
        }
        let piece = match value {
            Value::Text(text) if text.is_empty() => format!("{key}=\"…\""),
            Value::Text(text) => format!("{key}=\"{text}\""),
            Value::Raw(raw) => format!("{key}={raw}"),
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&piece);
        if out.chars().count() > UNKNOWN_SUMMARY_CAP {
            break;
        }
    }
    if out.chars().count() <= UNKNOWN_SUMMARY_CAP {
        return out;
    }
    let mut capped: String = out.chars().take(UNKNOWN_SUMMARY_CAP - 1).collect();
    capped.push('…');
    capped
}

/// One `key: value` line per argument, for an expanded unknown-tool card.
///
/// A non-string value is reported as `[invalid arg]` rather than as its raw JSON: the card's
/// job is to say what the argument *is*, and a raw token is the same blob the collapsed row
/// was already too long to read. `null` is the streaming state, so it prints the muted `...`
/// shape instead of pretending the argument is set.
pub(crate) fn format_tool_args_expanded(name: &str, args: &str) -> Vec<String> {
    if !is_unknown(name) {
        return Vec::new();
    }
    let fields = flat_object(args);
    if fields.is_empty() {
        return vec!["...".to_string()];
    }
    fields
        .into_iter()
        .map(|(key, value)| match value {
            Value::Text(text) if text.is_empty() => format!("{key}: ..."),
            Value::Text(text) => format!("{key}: {text}"),
            Value::Raw(raw) if raw == "null" => format!("{key}: ..."),
            Value::Raw(_) => format!("{key}: [invalid arg]"),
        })
        .collect()
}

/// Whether a tool has no purpose-built summary (the card's fallback grammar).
fn is_unknown(name: &str) -> bool {
    !matches!(
        name,
        "read" | "write" | "edit" | "list" | "ls" | "glob" | "grep" | "bash" | "shell"
    )
}

/// A shell call's timeout suffix: `(timeout 60s)`, or nothing when the call sets none.
///
/// `timeout` may be a JSON number or a numeric string for the same reason `offset` may be;
/// a non-numeric or non-positive value is treated as absent rather than printed as `0s`.
fn timeout_suffix(args: &str) -> String {
    let value = flat_object(args)
        .into_iter()
        .find(|(name, _)| name == "timeout")
        .map(|(_, value)| value);
    match value.as_ref().and_then(Value::number) {
        Some(seconds) => format!(" (timeout {seconds}s)"),
        None => String::new(),
    }
}

/// A one-line argument summary for a tool card (pi's per-tool formats, `messages.md` §6).
pub(crate) fn format_tool_args(name: &str, args: &str) -> String {
    if args.trim().is_empty() {
        // A call that has announced its name but no arguments yet: the card says so with the
        // muted `...` shape rather than rendering an empty span, which is indistinguishable
        // from a card that failed to paint (the issue's streaming-args gap).
        return "...".to_string();
    }
    match name {
        "read" => {
            let path = text_field(args, "path")
                .or_else(|| text_field(args, "file_path"))
                .unwrap_or_default();
            if path.is_empty() {
                return careful_fallback(args);
            }
            let fields = flat_object(args);
            let range = line_range(
                fields
                    .iter()
                    .find(|(key, _)| key == "offset")
                    .map(|(_, v)| v),
                fields
                    .iter()
                    .find(|(key, _)| key == "limit")
                    .map(|(_, v)| v),
            );
            match range {
                Some(range) => format!("{}:{range}", render_path(&path)),
                None => render_path(&path),
            }
        }
        "write" | "edit" | "list" | "ls" | "glob" => text_field(args, "path")
            .or_else(|| text_field(args, "file_path"))
            .map(|path| render_path(&path))
            .unwrap_or_else(|| careful_fallback(args)),
        "bash" | "shell" => match text_field(args, "command") {
            Some(command) => format!("{command}{}", timeout_suffix(args)),
            None => careful_fallback(args),
        },
        "grep" => match text_field(args, "pattern") {
            Some(pattern) => format!("/{pattern}/"),
            None => careful_fallback(args),
        },
        _ => unknown_summary(args),
    }
}

/// What a purpose-built tool's card shows when its own field is missing or not a string.
///
/// Not the raw JSON (that is the defect this file exists to fix): the card says the argument
/// is not usable, in the same `[invalid arg]` shape the fallback grammar uses, so the reader
/// learns the call is malformed rather than being handed a blob.
fn careful_fallback(args: &str) -> String {
    if flat_object(args).is_empty() {
        // Not an object at all: a streaming fragment or a producer that sent prose. Showing
        // it verbatim is the only honest thing left, and it is bounded.
        return unknown_summary(args);
    }
    "[invalid arg]".to_string()
}

#[cfg(test)]
#[path = "tool_args_tests.rs"]
mod tests;

// gh #405 gap 4: a tool argument's `\uXXXX` escapes must reach the card headline as text.
// Registered here because `format_tool_args` lives here (moved from `render.rs` by gh #392's
// ceiling split, gh #574's "keep the rows beside the code" rule).
#[cfg(test)]
#[path = "tool_args_escape_tests.rs"]
mod escape_tests;
