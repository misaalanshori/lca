//! How a shared stream run failed, in the vocabulary the core's
//! `ProviderError` speaks (`docs/headless.md`'s classes). One definition
//! for every extension on this kit (gh #189); the host never names it.

use lca_protocol::CapabilityError;

/// How a provider call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFailure {
    /// Human-readable message.
    pub message: String,
    /// Class for the headless envelope (`docs/headless.md`).
    pub class: &'static str,
    /// Whether a retry could help (FR-CORE-6).
    pub retryable: bool,
}

impl From<CapabilityError> for StreamFailure {
    fn from(err: CapabilityError) -> Self {
        use CapabilityError as E;
        // A refusal is a configuration problem, not a flaky network:
        // never retried, and its class keeps it out of "transport".
        let (class, retryable) = match &err {
            E::Permission(_) | E::NotGranted(_) | E::NotFound(_) | E::Invalid(_) => {
                ("invalid", false)
            }
            E::Io(_) | E::Timeout(_) => ("transport", true),
        };
        StreamFailure {
            message: err.to_string(),
            class,
            retryable,
        }
    }
}

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
