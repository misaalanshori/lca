//! Sensitive-value redaction (gh #258 §4, AGENTS.md Rule 6): secrets
//! never touch scrollback, panes, history — or log files. Every value
//! crossing into a trace passes through here first: header names,
//! schemes, and an identifying key prefix survive so developers can
//! tell *which* credential is active without exposing it.

/// Whether a key/parameter/cookie name carries a credential (gh #343).
///
/// Matched on a normalized name (lowercase, `_`/`-` stripped) so
/// `access_token`, `accessToken`, and `Access-Token` all match. Exact
/// names cover the short forms (`code`, `auth`, `session`); the suffix
/// rule covers the `*_token`/`*_key`/`*_secret`/`*_code`/`*_auth`
/// family.
///
/// Deliberately does NOT match plural usage counters (`max_tokens`,
/// `input_tokens`, `prompt_tokens`, `completion_tokens`) or `token_type` —
/// those are not credentials and keep wire logs readable.
///
/// Accepted over-mask (the direction is right: a false mask costs one
/// debug line, a false pass leaks a credential): `monkey`, `hotkey`, and
/// `zip_code`-style names mask too, because their normalized form ends in
/// a sensitive suffix.
fn is_sensitive_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|c| *c != '_' && *c != '-')
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    const EXACT: &[&str] = &[
        "key",
        "token",
        "code",
        "secret",
        "password",
        "authorization",
        "auth",
        "credentials",
        "session",
        "cookie",
        // The short session-cookie forms the suffix rule cannot reach.
        "sid",
        "sessid",
        "sessionid",
        "apikey",
    ];
    const SUFFIXES: &[&str] = &[
        "token",
        "key",
        "secret",
        "password",
        "code",
        "auth",
        "session",
        "credentials",
    ];
    EXACT.contains(&normalized.as_str()) || SUFFIXES.iter().any(|s| normalized.ends_with(s))
}

/// Mask a secret, keeping an identifying prefix: 8 chars for keys of
/// 16+ chars, the first half below that, nothing for a 0-1 char value.
pub fn redact_secret(secret: &str) -> String {
    let len = secret.chars().count();
    if len <= 1 {
        return "[REDACTED]".to_string();
    }
    let visible = if len >= 16 { 8 } else { len / 2 };
    let prefix: String = secret.chars().take(visible).collect();
    format!("{prefix}...[REDACTED]")
}

/// Render one HTTP header line with its value masked: the name always
/// survives, an auth scheme (`Bearer`, `Basic`) survives, cookies mask
/// per-cookie (sessions go, preferences stay), and anything else —
/// `content-type`, `user-agent` — passes through untouched so traces
/// stay readable.
pub fn redact_header(name: &str, value: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower == "cookie" || lower == "set-cookie" {
        return format!("{name}: {}", redact_cookie(value));
    }
    if lower == "authorization" || lower == "proxy-authorization" {
        if let Some((scheme, secret)) = value.split_once(char::is_whitespace) {
            let secret = secret.trim_start();
            if !secret.is_empty() {
                return format!("{name}: {scheme} {}", redact_secret(secret));
            }
        }
        return format!("{name}: {}", redact_secret(value));
    }
    if lower.contains("key") || lower.contains("token") || lower.contains("auth") {
        return format!("{name}: {}", redact_secret(value));
    }
    format!("{name}: {value}")
}

/// Mask a cookie header's session values, keeping preferences readable.
fn redact_cookie(value: &str) -> String {
    value
        .split(';')
        .map(|pair| {
            let pair = pair.trim();
            match pair.split_once('=') {
                Some((key, val)) => {
                    let key = key.trim();
                    if is_sensitive_key(key) {
                        format!("{key}={}", redact_secret(val.trim()))
                    } else {
                        pair.to_string()
                    }
                }
                None => pair.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Mask sensitive query parameters, keeping names and every other
/// parameter untouched.
pub fn redact_url(url: &str) -> String {
    let Some(query_start) = url.find('?') else {
        return url.to_string();
    };
    let (head, query) = url.split_at(query_start + 1);
    let masked = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, val)) => {
                if is_sensitive_key(key) {
                    format!("{key}={}", redact_secret(val))
                } else {
                    pair.to_string()
                }
            }
            None => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{head}{masked}")
}

/// Mask sensitive values in a JSON payload, preserving schema and keys
/// at every depth. Non-JSON input passes through (body previews are
/// often plain text).
pub fn redact_json(text: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(text) else {
        return text.to_string();
    };
    mask_json(&mut value);
    serde_json::to_string(&value).unwrap_or_else(|_| text.to_string())
}

/// Recursively mask sensitive object values.
fn mask_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if is_sensitive_key(key) {
                    match val {
                        serde_json::Value::String(text) => {
                            *val = serde_json::Value::String(redact_secret(text));
                        }
                        // A container under a sensitive name (e.g. `auth`)
                        // keeps its schema: recurse rather than blank the
                        // whole subtree, matching the module's "schema and
                        // keys survive at every depth" contract. Any secret
                        // nested inside is masked by the recursion.
                        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                            mask_json(val);
                        }
                        _ => {
                            *val = serde_json::Value::String("[REDACTED]".to_string());
                        }
                    }
                } else {
                    mask_json(val);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                mask_json(item);
            }
        }
        _ => {}
    }
}

/// One credential-store lookup line: the key name in full, the value
/// as a prefix only (`None` stays `None` — absence is not a secret).
pub fn redact_credential(label: &str, value: Option<&str>) -> String {
    match value {
        Some(secret) => format!(
            "get_credential(\"{label}\") -> Some({})",
            redact_secret(secret)
        ),
        None => format!("get_credential(\"{label}\") -> None"),
    }
}
