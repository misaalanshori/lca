//! The shared kit for subscription-gateway provider extensions (gh
//! #63): one OAuth PKCE login/refresh flow, parameterized by small
//! spec tables instead of branches. `extensions/codex` and
//! `extensions/grok` are thin specs over this; provider quirks live in
//! their tables, never in core (#157). The Responses wire protocol
//! this kit used to carry lives in `lca-wire-openai` now (gh #189).
//!
//! **The split (gh #567).** One file held four concerns; each now has its own
//! private module: `spec` is the data (the `OAuthSpec` table and the
//! credential-key readers), `crypto` the PKCE/base64url primitives,
//! `transport` the bounded form/JSON POSTs and the clock, `token` the credential
//! lifecycle and account extraction, `login` the loopback flow, `device` RFC
//! 8628. Every public path is re-exported from the crate root below, so a
//! caller's `lca_subscription::run_login` did not move.

pub mod wasm;

mod crypto;
mod device;
mod login;
mod spec;
mod token;
mod transport;

use lca_protocol::StreamFailure;

/// Re-exported so extensions name the outcome in their signatures.
pub use lca_protocol::IdentityOutcome;

/// The kit's error helpers, still used by the token calls below.
pub use lca_wire_openai::json_error_message;

pub use crypto::{percent_encode, pkce_pair};
pub use device::{
    DeviceAuthorization, DevicePollOutcome, device_code_url, open_failure_event, poll_device_code,
    poll_device_once, request_device_code,
};
pub use login::{run_login, run_logout};
pub use spec::{AccountStrategy, OAuthSpec};
pub use token::{access_token, account_from_jwt, purge_tokens};
pub use transport::{now_epoch, post_form_accept, post_json, post_json_with_headers};

/// An identity operation failed; the string is shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityFailure(pub String);

impl From<StreamFailure> for IdentityFailure {
    fn from(err: StreamFailure) -> Self {
        IdentityFailure(err.message)
    }
}

impl From<IdentityFailure> for StreamFailure {
    fn from(err: IdentityFailure) -> Self {
        StreamFailure {
            message: err.0,
            class: "invalid",
            retryable: false,
        }
    }
}
