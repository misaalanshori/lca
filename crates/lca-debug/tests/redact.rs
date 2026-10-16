//! Redaction guards (gh #258 §4, acceptance 7): secrets never emit.
//! Written first — nothing traces before this holds.

use lca_debug::redact::{redact_credential, redact_header, redact_json, redact_secret, redact_url};

// Verifies: gh #258 - the prefix rule (8 chars at 16+, half below,
// nothing at 0-1).
#[test]
fn secret_prefixes_follow_the_length_rule() {
    assert_eq!(redact_secret("ya29.a0AfH6SM12345"), "ya29.a0A...[REDACTED]");
    assert_eq!(
        redact_secret("AIzaSyDx9876543210abc"),
        "AIzaSyDx...[REDACTED]"
    );
    assert_eq!(redact_secret("sk-ant-a"), "sk-a...[REDACTED]");
    assert_eq!(redact_secret("ab"), "a...[REDACTED]");
    assert_eq!(redact_secret("x"), "[REDACTED]");
    assert_eq!(redact_secret(""), "[REDACTED]");
}

// Verifies: gh #258 - header names and schemes survive, values do not.
#[test]
fn header_names_and_schemes_survive() {
    assert_eq!(
        redact_header("Authorization", "Bearer ya29.a0AfH6SM12345"),
        "Authorization: Bearer ya29.a0A...[REDACTED]"
    );
    assert_eq!(
        redact_header("Authorization", "Basic dXNlcjpwYXNzd29yZA"),
        "Authorization: Basic dXNlcjpw...[REDACTED]"
    );
    assert_eq!(
        redact_header("x-goog-api-key", "AIzaSyDx9876543210abc"),
        "x-goog-api-key: AIzaSyDx...[REDACTED]"
    );
    assert_eq!(
        redact_header("api-key", "sk-ant-api03-abcdef123456"),
        "api-key: sk-ant-a...[REDACTED]"
    );
    assert_eq!(
        redact_header("content-type", "application/json"),
        "content-type: application/json"
    );
}

// Verifies: gh #258 - sessions mask, preferences stay.
#[test]
fn cookies_mask_sessions_keep_preferences() {
    assert_eq!(
        redact_header("cookie", "session_id=abc123456789xyz; theme=dark"),
        "cookie: session_id=abc1234...[REDACTED]; theme=dark"
    );
}

// Verifies: gh #258 - parameter names and innocent parameters survive.
#[test]
fn urls_mask_sensitive_parameters() {
    assert_eq!(
        redact_url("https://x.test/v1?key=AIzaSyDx9876543210abc&model=xyz"),
        "https://x.test/v1?key=AIzaSyDx...[REDACTED]&model=xyz"
    );
    assert_eq!(
        redact_url("https://x.test/v1?token=ya29.a0AfH6SM12345&page=2"),
        "https://x.test/v1?token=ya29.a0A...[REDACTED]&page=2"
    );
    assert_eq!(
        redact_url("https://x.test/v1?model=xyz"),
        "https://x.test/v1?model=xyz"
    );
    assert_eq!(redact_url("https://x.test/plain"), "https://x.test/plain");
}

// Verifies: gh #258 - schema and keys survive at every depth,
// values do not; non-JSON passes through.
#[test]
fn json_masks_values_keeps_schema() {
    assert_eq!(
        redact_json(r#"{"key": "AIzaSyDx9876543210abc", "model": "gemini-2.5-flash"}"#),
        r#"{"key":"AIzaSyDx...[REDACTED]","model":"gemini-2.5-flash"}"#
    );
    assert_eq!(
        redact_json(r#"{"auth": {"refresh_token": "1//0gabcdef123456"}}"#),
        r#"{"auth":{"refresh_token":"1//0gabc...[REDACTED]"}}"#
    );
    assert_eq!(redact_json("plain text"), "plain text");
}

// Verifies: gh #258 - credential lookups log the key and a prefix.
#[test]
fn credential_lookups_log_key_and_prefix() {
    assert_eq!(
        redact_credential("antigravity:oauth_token", Some("ya29.a0AfH6SM12345")),
        "get_credential(\"antigravity:oauth_token\") -> Some(ya29.a0A...[REDACTED])"
    );
    assert_eq!(
        redact_credential("missing", None),
        "get_credential(\"missing\") -> None"
    );
}

// ---------------------------------------------------------------------------
// gh #343: the exact-match list let standard OAuth keys through. One shared
// normalized matcher (lowercase, `_`/`-` stripped) with exact + suffix rules.
// ---------------------------------------------------------------------------

// Verifies: gh #343 acceptance 1 - OAuth token-exchange payloads mask every
// credential key (the live leak shape: `access_token`/`id_token` in
// cleartext), in both snake_case and camelCase.
#[test]
fn oauth_token_exchange_payloads_mask_every_credential_key() {
    let out = redact_json(
        r#"{"access_token":"ya29.a0AfH6SM12345","id_token":"eyJhbGciOiJSUzI1NiJ9.abc","refresh_token":"1//0gabcdef123456","client_secret":"GOCSPX-abcdef123","expires_in":3600,"token_type":"Bearer"}"#,
    );
    for secret in [
        "ya29.a0AfH6SM12345",
        "eyJhbGciOiJSUzI1NiJ9.abc",
        "1//0gabcdef123456",
        "GOCSPX-abcdef123",
    ] {
        assert!(!out.contains(secret), "{secret} leaked in {out}");
    }
    assert!(out.contains("access_token"), "the key survives: {out}");
    assert!(out.contains("expires_in"));
    // `token_type` is not a credential: it stays visible (readability pin).
    assert!(out.contains(r#""token_type":"Bearer"#));

    // camelCase and hyphenated spellings mask the same way.
    assert!(!redact_json(r#"{"accessToken":"FAKESECRET-CAMEL-1"}"#).contains("FAKESECRET-CAMEL-1"));
    assert!(
        !redact_json(r#"{"Access-Token":"FAKESECRET-HYPHEN-1"}"#).contains("FAKESECRET-HYPHEN-1")
    );
}

// Verifies: gh #343 acceptance 2 - an OAuth loopback callback URL masks its
// one-time `code` (and the other credential params).
#[test]
fn oauth_callback_urls_mask_the_code_and_tokens() {
    let out = redact_url(
        "http://127.0.0.1:8080/callback?code=4/0AfH6SM12345&state=xyz&token=fa29.a0AfH6SM12345",
    );
    assert!(!out.contains("4/0AfH6SM12345"), "code leaked in {out}");
    assert!(!out.contains("fa29.a0AfH6SM12345"), "token leaked in {out}");
    assert!(
        out.contains("state=xyz"),
        "state is not a credential: {out}"
    );
}

// Verifies: gh #343 - camelCase cookie names mask (the snake-case list
// missed `sessionId`), while the pinned `session_id` behavior is kept.
#[test]
fn camel_case_session_cookies_mask() {
    assert_eq!(
        redact_header("cookie", "sessionId=abc123456789xyz; theme=dark"),
        "cookie: sessionId=abc1234...[REDACTED]; theme=dark"
    );
    // The pre-#343 pinned shape still holds.
    assert_eq!(
        redact_header("cookie", "session_id=abc123456789xyz; theme=dark"),
        "cookie: session_id=abc1234...[REDACTED]; theme=dark"
    );
    assert_eq!(
        redact_header("cookie", "sid=abc123456789xyz; sessid=def123456789xyz"),
        "cookie: sid=abc1234...[REDACTED]; sessid=def1234...[REDACTED]"
    );
}

// Verifies: gh #343 - usage counters and `token_type` stay visible (the
// deliberate non-mask that keeps wire logs readable), while the documented
// over-mask trade (`monkey`, `zip_code`) masks.
#[test]
fn usage_counters_stay_visible_and_the_over_mask_is_deliberate() {
    let payload = r#"{"max_tokens":4096,"input_tokens":123,"prompt_tokens":100,"completion_tokens":23,"token_type":"Bearer","model":"m"}"#;
    let out = redact_json(payload);
    for visible in [
        "\"max_tokens\":4096",
        "\"input_tokens\":123",
        "\"prompt_tokens\":100",
        "\"completion_tokens\":23",
        "\"token_type\":\"Bearer\"",
        "\"model\":\"m\"",
    ] {
        assert!(
            out.contains(visible),
            "{visible} should stay visible in {out}"
        );
    }
    assert!(!out.contains("REDACTED"), "no counter is a secret: {out}");
    // The accepted over-mask: a false mask costs a debug line; a false pass
    // leaks a credential. Pinned so the trade is visible and deliberate.
    assert!(!redact_json(r#"{"monkey":"FAKESECRET-MONKEY-1"}"#).contains("FAKESECRET-MONKEY-1"));
    assert!(!redact_json(r#"{"zip_code":"FAKESECRET-ZIP-1"}"#).contains("FAKESECRET-ZIP-1"));
}

// Verifies: gh #343 - a URL with no query and non-JSON text pass through
// unchanged (the pre-existing passthrough contract).
#[test]
fn no_query_and_non_json_pass_through() {
    assert_eq!(redact_url("https://x.test/plain"), "https://x.test/plain");
    assert_eq!(redact_json("plain text"), "plain text");
}
