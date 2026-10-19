//! Regression rows for gh #319: the loopback OAuth redirect server and the
//! callback query decoder.
//!
//! Two defects in one flow:
//!
//! 1. `net.rs` answered the browser with a malformed HTTP/1.1 response — 36
//!    leading spaces on the `content-length` and `<html>` lines, bare `\n`
//!    line endings, and `content-length:63` for a 104-byte body. A browser
//!    pointed at it either hung waiting for the announced 63 bytes or
//!    interpreted the leading whitespace as an obsolete line fold.
//! 2. `decode()` percent-decoded into a `String` via `char::from(byte)`,
//!    which widens each raw byte as Latin-1. `%C3%A9` (`é`) became `Ã©`,
//!    and every non-ASCII byte copied through the `other =>` arm was
//!    corrupted the same way.
//!
//! Verifies: gh #319 - a conforming HTTP/1.1 reply and UTF-8 query decoding.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The announced length must equal the body's real length, and the
/// response must be `\r\n`-framed with no leading whitespace on any line.
#[test]
fn the_loopback_reply_is_a_well_formed_http_1_1_response() {
    let page = lca_tools::loopback_oauth_page();
    assert!(
        page.starts_with("HTTP/1.1 200 OK\r\n"),
        "the status line is first and CRLF-terminated: {:?}",
        &page[..page.len().min(40)]
    );
    let (head, body) = page
        .split_once("\r\n\r\n")
        .expect("the header block ends with a blank CRLF line");
    // No bare LF anywhere in the header block: every line ends CRLF, and
    // CRLF is not doubled.
    assert!(
        !head.replace("\r\n", "").contains('\n'),
        "every header line ends CRLF: {head:?}"
    );
    for line in head.lines() {
        assert!(
            !line.starts_with(' '),
            "no header line begins with whitespace (obsolete folding): {line:?}"
        );
    }
    let announced = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .expect("a Content-Length header is present");
    assert_eq!(
        announced,
        body.len(),
        "Content-Length matches the body it announces ({} vs {})",
        announced,
        body.len()
    );
    assert!(
        !body.starts_with(' ') && !body.starts_with('\n'),
        "the body begins immediately after the blank line: {body:?}"
    );
    assert!(
        body.contains("</html>"),
        "the body is a complete HTML document: {body:?}"
    );
}

/// A percent-encoded UTF-8 sequence decodes to the character it encodes.
#[test]
fn a_percent_encoded_utf8_parameter_decodes_to_its_character() {
    let decoded = lca_tools::decode_query_component("caf%C3%A9");
    assert_eq!(decoded, "café", "two-byte UTF-8 survives the decoder");
    let decoded = lca_tools::decode_query_component("%E6%BC%A2%E5%AD%97");
    assert_eq!(decoded, "漢字", "three-byte UTF-8 survives the decoder");
    let decoded = lca_tools::decode_query_component("%F0%9F%98%80");
    assert_eq!(decoded, "😀", "four-byte UTF-8 survives the decoder");
}

/// Raw non-ASCII bytes that arrive unencoded must not be widened either.
/// The old `other => out.push(other as char)` arm turned every byte of a
/// literal `é` into its own Latin-1 character.
#[test]
fn an_unencoded_utf8_parameter_is_not_widened_as_latin_1() {
    assert_eq!(
        lca_tools::decode_query_component("café"),
        "café",
        "a literal multi-byte character passes through unchanged"
    );
    assert_eq!(
        lca_tools::decode_query_component("漢字"),
        "漢字",
        "a literal three-byte character passes through unchanged"
    );
}

/// `+` is a space, and `%20` is a space; the same key uses both.
#[test]
fn plus_and_percent_twenty_both_mean_space() {
    assert_eq!(lca_tools::decode_query_component("a+b%20c"), "a b c");
}

/// A `%` that is not followed by two hex digits must be preserved
/// literally, not turned into a NUL byte. The old nibble helper mapped any
/// non-hex character to `0`, so `%zz` became `\0` and `%4` at the end of
/// the string was silently dropped.
#[test]
fn a_malformed_percent_sequence_is_preserved_verbatim() {
    for (input, expected) in [
        ("%zz", "%zz"),
        ("%4", "%4"),
        ("%", "%"),
        ("a%", "a%"),
        ("100%25", "100%"),
        ("%G1", "%G1"),
        ("%1G", "%1G"),
    ] {
        let decoded = lca_tools::decode_query_component(input);
        assert_eq!(decoded, expected, "malformed sequence {input:?}");
        assert!(
            !decoded.contains('\0'),
            "no NUL is invented for {input:?}: {decoded:?}"
        );
    }
}

/// The full parse: keys and values both decode, empty values survive, and a
/// pair without `=` keeps an empty value.
#[test]
fn query_parsing_decodes_keys_and_values_alike() {
    let params = lca_tools::parse_callback_query("code=abc%20def&st%C3%A4te=x+y&flag");
    assert_eq!(
        params,
        vec![
            ("code".to_string(), "abc def".to_string()),
            ("stäte".to_string(), "x y".to_string()),
            ("flag".to_string(), String::new()),
        ]
    );
}

/// A state parameter carrying a base64url blob with `=` padding survives
/// end to end — the shape the OAuth flow actually sends.
#[test]
fn a_state_parameter_survives_the_round_trip() {
    let params = lca_tools::parse_callback_query("code=0%2FAb%2B-c&state=eyJhIjoiYiJ9%3D%3D");
    assert_eq!(
        params,
        vec![
            ("code".to_string(), "0/Ab+-c".to_string()),
            ("state".to_string(), "eyJhIjoiYiJ9==".to_string()),
        ]
    );
}
