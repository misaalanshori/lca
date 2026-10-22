//! The inbound HTTP capability's contract types (gh #363, ADR-0049).
//!
//! These live in `lca-protocol` rather than in `lca-tools` because three crates
//! have to name them and only one direction of dependency exists:
//! `lca-tools` owns the listener, `lca-ext-host` owns the guest call, and
//! `lca-ext-abi`'s dispatch trait would have to name the mount. Putting the
//! vocabulary at the bottom means the listeners and the callers share one
//! definition instead of one crate reaching up into another.
//!
//! The WIT side (`wit/world-serve-http.wit`) carries the same shape for a
//! component. It is deliberately a separate declaration rather than a
//! generated one: the guest's `request`/`response` records are the ABI promise,
//! and a change here must be matched there on purpose, not inherited.

use crate::CapabilityError;

/// One request the host's listener read off the wire, addressed to one
/// extension.
///
/// `path` is the **stripped** path: the host has already removed
/// `/<session-id>/<extension-name>`, so it always begins with `/`. That is the
/// contract that lets an extension serve `index.html` and `./style.css`
/// without knowing where it is mounted (ADR-0049's decision 3).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpRequest {
    /// Upper-case method as sent (`GET`, `POST`, ...).
    pub method: String,
    /// The stripped path, always starting with `/`.
    pub path: String,
    /// Everything after `?`, without the `?`; empty when absent.
    pub query: String,
    /// Headers with names lower-cased. The auth token is **not** among them:
    /// the host consumes it before the request reaches a guest, so an extension
    /// cannot see, log, or forward the one secret guarding its own port.
    pub headers: Vec<(String, String)>,
    /// The body, already bounded by the host's limit.
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// The first value for `name`, case-insensitively.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| value.as_str())
    }
}

/// What a guest answers with.
///
/// `content-length` and `connection` are the host's, not the guest's: the host
/// owns the socket, so it owns the framing that tells a client how much to
/// read. A guest that set its own length could hang a browser by lying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Headers as `(name, value)`.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A small plain-text response, used by the host for its own refusals and
    /// for a guest that failed.
    #[must_use]
    pub fn text(status: u16, body: impl Into<String>) -> HttpResponse {
        HttpResponse {
            status,
            headers: vec![(
                "content-type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            )],
            body: body.into().into_bytes(),
        }
    }
}

/// One extension mounted on the host's listener.
///
/// Implemented by the host for a loaded component, and by a test stub where the
/// point is the host's gate rather than a guest's behaviour. `Send + Sync`
/// because the accept loop is its own thread and every connection is served on
/// another.
pub trait ServeHttpNamespace: Send + Sync {
    /// The extension name this namespace serves.
    fn extension(&self) -> &str;
    /// The session id this namespace is mounted under.
    fn session(&self) -> &str;
    /// Answer one request. `request.path` is already stripped and starts `/`.
    fn answer(&self, base_url: &str, request: HttpRequest)
    -> Result<HttpResponse, CapabilityError>;
    /// Record a refusal against this extension's denial journal.
    fn record_refusal(&self, parameter: &str, reason: &str);
}
