//! Capability-call errors shared by every delivery mode: the WASM host
//! maps these onto the WIT error variants, the native path returns them
//! directly, and both must produce identical text (conformance diff).

use std::fmt;

/// Why a capability call was refused or failed (capability catalog's
/// denial shapes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityError {
    /// The operation was refused: scope escape, state directory, mode,
    /// or the user declined the prompt.
    Permission(String),
    /// The capability or scope is not in the granted set (NFR-13).
    NotGranted(String),
    /// A file, scope entry, or handle does not exist.
    NotFound(String),
    /// The operating system refused.
    Io(String),
    /// The guest supplied something malformed (unknown handle, absolute
    /// path, bad dimensions).
    Invalid(String),
}

impl CapabilityError {
    /// The stable, deterministic text both delivery modes put in results.
    pub fn text(&self) -> String {
        format!("capability error: {self}")
    }
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapabilityError::Permission(detail) => write!(f, "permission denied: {detail}"),
            CapabilityError::NotGranted(detail) => write!(f, "not granted: {detail}"),
            CapabilityError::NotFound(detail) => write!(f, "not found: {detail}"),
            CapabilityError::Io(detail) => write!(f, "i/o error: {detail}"),
            CapabilityError::Invalid(detail) => write!(f, "invalid argument: {detail}"),
        }
    }
}

impl std::error::Error for CapabilityError {}

impl From<std::io::Error> for CapabilityError {
    fn from(err: std::io::Error) -> CapabilityError {
        match err.kind() {
            std::io::ErrorKind::NotFound => CapabilityError::NotFound(err.to_string()),
            _ => CapabilityError::Io(err.to_string()),
        }
    }
}
