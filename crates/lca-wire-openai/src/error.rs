//! The OpenAI-family failure vocabulary: the status classification and the
//! vendor-envelope message, beside a re-export of the **one**
//! [`lca_protocol::StreamFailure`] every kit now shares.
//!
//! Gh #576 moved the type itself to `lca-protocol`, next to `CapabilityError`,
//! because two kits defined it and `lca-subscription` depended on this kit only
//! to name it. The re-export keeps every existing path
//! (`lca_wire_openai::StreamFailure`, `lca_wire_openai::error::StreamFailure`)
//! working; the fields and the one `From<CapabilityError>` live there now.

pub use lca_protocol::StreamFailure;

/// Whether an HTTP status is worth retrying (FR-CORE-6).
pub fn classify_status(status: u16) -> bool {
    status == 429 || status == 408 || (500..=599).contains(&status)
}

/// Classify an HTTP status into the headless envelope's classes.
pub fn failure_for_status(status: u16, detail: &str) -> StreamFailure {
    let class = match status {
        401 | 403 => "auth",
        400..=499 => "invalid",
        _ => "transport",
    };
    StreamFailure {
        message: format!("provider returned HTTP {status}: {detail}"),
        class,
        retryable: classify_status(status),
    }
}

/// The one-line error inside a vendor JSON error envelope, else the
/// first 200 chars of the body.
pub fn json_error_message(text: &str) -> String {
    let json: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    json.get("error")
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let text = text.trim();
            if text.is_empty() {
                "unknown error".to_string()
            } else {
                text.chars().take(200).collect()
            }
        })
}
