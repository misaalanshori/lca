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

/// Wrap bare `http://`/`https://` runs in OSC 8 hyperlinks (gh
/// #178): modals show raw URLs, and only an explicit sequence keeps a
/// wrapped URL clickable whole. Lines that already carry a sequence
/// are left alone (explicit links win); a trailing run of sentence
/// punctuation stays outside the link.
pub fn linkify_urls(text: &str) -> String {
    if text.contains("\x1b]8;") || (!text.contains("http://") && !text.contains("https://")) {
        return text.to_string();
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(found) = rest.find("http") {
        let after = &rest[found..];
        let scheme = if after.starts_with("http://") {
            "http://"
        } else if after.starts_with("https://") {
            "https://"
        } else {
            out.push_str(&rest[..found + 4]);
            rest = &rest[found + 4..];
            continue;
        };
        let boundary = found == 0
            || matches!(
                rest[..found].chars().next_back(),
                Some(c) if c.is_whitespace() || matches!(c, '(' | '"' | '\'')
            );
        if !boundary {
            out.push_str(&rest[..found + 4]);
            rest = &rest[found + 4..];
            continue;
        }
        let tail = &rest[found..];
        let end = tail.find(|c: char| c.is_whitespace()).unwrap_or(tail.len());
        let mut url = &tail[..end];
        while url.len() > scheme.len()
            && matches!(
                url.as_bytes()[url.len() - 1],
                b'.' | b',' | b';' | b':' | b'!' | b'?' | b')' | b']' | b'\'' | b'"'
            )
        {
            url = &url[..url.len() - 1];
        }
        out.push_str(&rest[..found]);
        out.push_str(&format!("\x1b]8;;{url}\x07{url}\x1b]8;;\x07"));
        rest = &tail[url.len()..];
    }
    out.push_str(rest);
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #178 - bare URLs linkify whole; explicit sequences
    // and sentence punctuation are left alone.
    #[test]
    fn linkify_wraps_bare_urls_and_leaves_the_rest() {
        let linked = linkify_urls("see https://pi.dev/x?a=1&b=2 for details");
        assert_eq!(
            linked,
            "see \x1b]8;;https://pi.dev/x?a=1&b=2\x07https://pi.dev/x?a=1&b=2\x1b]8;;\x07 for details"
        );
        assert_eq!(linkify_urls("no links here"), "no links here");
        let already = "\x1b]8;;https://pi.dev\x07pi\x1b]8;;\x07";
        assert_eq!(linkify_urls(already), already);
        let punct = linkify_urls("(see https://pi.dev/x.)");
        assert!(
            punct.ends_with("\x1b]8;;\x07.)"),
            "the sentence punctuation stays outside: {punct:?}"
        );
    }

    #[test]
    fn an_osc8_open_parses_and_a_close_is_none() {
        let open = parse_osc8_hyperlink("\x1b]8;;https://pi.dev\x07")
            .expect("an open")
            .expect("a link");
        assert_eq!(open.url, "https://pi.dev");
        assert_eq!(open.terminator, Osc8Terminator::Bel);
        assert_eq!(parse_osc8_hyperlink("\x1b]8;;\x07"), Some(None));
        assert!(parse_osc8_hyperlink("nope").is_none());
    }
}
