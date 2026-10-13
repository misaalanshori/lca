//! Sensitive-value redaction (gh #258 §4, AGENTS.md Rule 6): secrets
//! never touch scrollback, panes, history — or log files. Every value
//! crossing into a trace passes through here first: header names,
//! schemes, and an identifying key prefix survive so developers can
//! tell *which* credential is active without exposing it.

/// Sensitive JSON keys and query parameters (the spec's set, verbatim).
const SENSITIVE_KEYS: &[&str] = &[
    "key",
    "token",
    "client_secret",
    "refresh_token",
    "api_key",
    "password",
];

/// Cookie names whose values are sessions, not preferences.
const SENSITIVE_COOKIES: &[&str] = &[
    "session",
    "session_id",
    "token",
    "auth",
    "sid",
    "sessid",
    "credentials",
    "secret",
    "password",
    "api_key",
    "apikey",
];

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
                    if SENSITIVE_COOKIES
                        .iter()
                        .any(|sensitive| key.eq_ignore_ascii_case(sensitive))
                    {
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
                if SENSITIVE_KEYS
                    .iter()
                    .any(|sensitive| key.eq_ignore_ascii_case(sensitive))
                {
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
                if SENSITIVE_KEYS
                    .iter()
                    .any(|sensitive| key.eq_ignore_ascii_case(sensitive))
                {
                    if let Some(text) = val.as_str() {
                        *val = serde_json::Value::String(redact_secret(text));
                    } else {
                        *val = serde_json::Value::String("[REDACTED]".to_string());
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
