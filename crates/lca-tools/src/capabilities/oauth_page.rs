//! The loopback OAuth flow's two pure units: the HTTP/1.1 reply the
//! redirect server answers with, and the callback query decoder (gh #319).
//!
//! Split out of `capabilities/mod.rs` for the 1200-line ceiling. Both are
//! pure functions over strings, which is what makes them testable without
//! standing up a socket - the reason the malformed reply literal survived
//! for so long is that nothing could assert on it.

/// Parse a callback query string into pairs (the extension verifies
/// `state` itself; the host only parses, per `docs/flows.md`).
///
/// Both halves of a pair are percent-decoded when the extension receives
/// them, because a provider may encode either side (gh #319).
#[must_use]
pub fn parse_callback_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (decode_query_component(key), decode_query_component(value)),
            None => (decode_query_component(pair), String::new()),
        })
        .collect()
}

/// The page the loopback OAuth redirect server answers with (gh #319).
///
/// A conforming HTTP/1.1 response: CRLF-framed headers, no leading
/// whitespace on any line, and a `Content-Length` that matches the body it
/// announces. The previous literal carried 36 leading spaces on two lines,
/// bare `\n` endings, and a length of 63 for a 104-byte body, so a browser
/// either hung waiting for bytes that never came or read the indentation as
/// obsolete header folding.
///
/// Kept as a function rather than a literal so the framing is a testable
/// unit instead of a string nobody can assert on.
#[must_use]
pub fn loopback_oauth_page() -> String {
    const BODY: &str =
        "<!DOCTYPE html><html><body>You can close this tab and return to LCA.</body></html>";
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        BODY.len(),
        BODY
    )
}

/// Percent-decode one query component as UTF-8 (gh #319).
///
/// Bytes are accumulated first and decoded once at the end: decoding each
/// byte to a `char` as it arrived widened every raw byte as Latin-1, so
/// `%C3%A9` (`é`) came back as `Ã©` and a literal multi-byte character was
/// mangled the same way. A `%` that is not followed by two hex digits is
/// preserved verbatim rather than becoming a NUL byte, so a malformed
/// sequence degrades to text instead of silently corrupting the value.
#[must_use]
pub fn decode_query_component(value: &str) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'+' => out.push(b' '),
            b'%' => {
                let hi = bytes.next();
                let lo = bytes.next();
                match (hi, lo) {
                    (Some(hi), Some(lo)) => match (hex_value(hi), hex_value(lo)) {
                        (Some(hi), Some(lo)) => out.push((hi << 4) | lo),
                        // Not a percent escape: preserve all three bytes
                        // rather than inventing a NUL for the bad digits.
                        _ => out.extend_from_slice(&[b'%', hi, lo]),
                    },
                    // Truncated at end of string: keep the bytes that were
                    // there, so `%4` stays `%4` and `%` stays `%`.
                    (Some(hi), None) => out.extend_from_slice(&[b'%', hi]),
                    (None, _) => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The value of one hex digit, or `None` when the byte is not one.
pub(crate) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
