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
