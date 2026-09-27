//! The capability engine's public error types (cycle 7, P3 split).

/// Why a host-mediated `completion` call produced no text (the extension asks
///, the host routes to the active provider; ADR-0008).
#[derive(Debug, thiserror::Error)]
pub enum CompletionError {
    /// The active provider failed (transport, auth, protocol).
    #[error("{0}")]
    Provider(String),
    /// No provider is currently active.
    #[error("no active provider is available")]
    NoProvider,
}

/// Why opening a URL in the user's browser failed (the OAuth flow's
/// `oauth.open`). Opaque at this boundary - the platform launcher's error.
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    /// The platform launcher refused or failed.
    #[error("{0}")]
    Launch(String),
}
