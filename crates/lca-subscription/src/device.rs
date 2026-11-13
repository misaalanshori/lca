//! The RFC 8628 device-authorization flow (gh #567: split out of `lib.rs` by
//! OAuth concern).

use lca_protocol::{ProviderCap, StreamFailure};

use crate::IdentityFailure;
use crate::json_error_message;
use crate::transport::post_form_accept;

/// One RFC 8628 device authorization (gh #184, #186, #187): the
/// code the user types at the verification page, and the knobs the
/// poll loop runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeviceAuthorization {
    /// The `device_code` the token poll redeems.
    pub device_code: String,
    /// The `user_code` the person types.
    pub user_code: String,
    /// Where they type it (`verification_uri_complete` when the
    /// server sends one, else `verification_uri`).
    pub verification_uri: String,
    /// Seconds between polls (the server's `interval`, else 5 —
    /// RFC 8628 section 3.2).
    pub interval_secs: u64,
    /// The poll loop's own expiry (the server's `expires_in`, else a
    /// 15-minute client ceiling).
    pub expires_in_secs: u64,
}

impl DeviceAuthorization {
    /// One device authorization, every field named (gh #567).
    ///
    /// The struct is `#[non_exhaustive]` so RFC 8628's optional
    /// `verification_uri_complete` can be added later without breaking a caller;
    /// this constructor is the way to build one in a test or a fixture.
    #[must_use]
    pub fn new(
        device_code: impl Into<String>,
        user_code: impl Into<String>,
        verification_uri: impl Into<String>,
        interval_secs: u64,
        expires_in_secs: u64,
    ) -> Self {
        DeviceAuthorization {
            device_code: device_code.into(),
            user_code: user_code.into(),
            verification_uri: verification_uri.into(),
            interval_secs,
            expires_in_secs,
        }
    }
}

/// Start one device authorization: form-POST the client (plus scope
/// where the gateway wants one) and validate the answer. The
/// verification URI must be http(s) — it opens in the user's browser
/// (pi's `trustedHttpUrl`).
pub fn request_device_code(
    cap: &dyn ProviderCap,
    url: &str,
    client_id: &str,
    scope: Option<&str>,
) -> Result<DeviceAuthorization, IdentityFailure> {
    let mut pairs = vec![("client_id", client_id)];
    if let Some(scope) = scope {
        pairs.push(("scope", scope));
    }
    let (status, text) = post_form_accept(cap, url, &pairs, "application/json")?;
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "device authorization failed: {}",
            json_error_message(&text)
        )));
    }
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| IdentityFailure("invalid device code response".to_string()))?;
    let get = |key: &str| {
        json.get(key)
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string()
    };
    let device_code = get("device_code");
    let user_code = get("user_code");
    if device_code.is_empty() || user_code.is_empty() {
        return Err(IdentityFailure(
            "invalid device code response fields".to_string(),
        ));
    }
    let complete = get("verification_uri_complete");
    let plain = get("verification_uri");
    let verification_uri = if is_http_url(&complete) {
        complete
    } else if is_http_url(&plain) {
        plain
    } else {
        return Err(IdentityFailure(
            "untrusted verification_uri in device code response".to_string(),
        ));
    };
    Ok(DeviceAuthorization::new(
        device_code,
        user_code,
        verification_uri,
        json.get("interval")
            .and_then(|value| value.as_u64())
            .filter(|interval| *interval > 0)
            .unwrap_or(5),
        json.get("expires_in")
            .and_then(|value| value.as_u64())
            .filter(|expires| *expires > 0)
            .unwrap_or(900),
    ))
}

/// Only http(s) URLs open in the user's browser.
fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

/// The URL the extension asks the host to open for one device
/// authorization (gh #184): the verification page with the user code
/// in the fragment. Fragments never reach the server, so navigation
/// is unaffected; the host splits the code back off for display (the
/// only freeze-safe channel — no new host import).
pub fn device_code_url(authorization: &DeviceAuthorization) -> String {
    format!(
        "{}#code={}",
        authorization.verification_uri, authorization.user_code
    )
}

/// One device-token poll's verdict (pi's `device-code.ts` statuses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePollOutcome {
    /// The server answered a token: the whole JSON body.
    Complete(serde_json::Value),
    /// `authorization_pending`: keep polling.
    Pending,
    /// `slow_down`: wait longer (the server's new `interval`, if valid).
    SlowDown(Option<u64>),
    /// Anything else: the message ends the flow.
    Failed(String),
}

/// POST one device-token poll and map the answer (all three gateways
/// share the mapping: `access_token` wins, the two wait codes wait,
/// everything else — including a transport-shaped 5xx — ends it).
pub fn poll_device_once(
    cap: &dyn ProviderCap,
    url: &str,
    pairs: &[(&str, &str)],
) -> Result<DevicePollOutcome, IdentityFailure> {
    let (status, text) = post_form_accept(cap, url, pairs, "application/json")?;
    let json: serde_json::Value = serde_json::from_str(&text).ok().unwrap_or_default();
    if (200..300).contains(&status)
        && let Some(token) = json.get("access_token").and_then(|value| value.as_str())
        && !token.is_empty()
    {
        return Ok(DevicePollOutcome::Complete(json));
    }
    let error = json
        .get("error")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    match error {
        "authorization_pending" => Ok(DevicePollOutcome::Pending),
        "slow_down" => Ok(DevicePollOutcome::SlowDown(
            json.get("interval")
                .and_then(|value| value.as_u64())
                .filter(|interval| *interval > 0),
        )),
        _ => {
            let description = json
                .get("error_description")
                .and_then(|value| value.as_str())
                .map(|description| format!(": {description}"))
                .unwrap_or_default();
            let message = if error.is_empty() {
                format!("device token request failed with status {status}")
            } else {
                format!("device flow failed: {error}{description}")
            };
            Ok(DevicePollOutcome::Failed(message))
        }
    }
}

/// The `(message, retryable)` pair a failed stream open is delivered as, on any
/// delivery mode (gh #566).
///
/// The WIT `stream-event` case `error` carries a message and a retryability
/// flag, and the host maps it straight onto `StreamEvent::Error`. `class` has no
/// slot in that record, so what crosses is exactly these two facts - the ones
/// the retry rule needs. The guest returns the failure as the stream's own
/// event (rather than as the `result<_, string>` the signature also allows)
/// precisely so this flag travels **verbatim**: returned as a string error, the
/// host would re-derive retryability from the message text (the gh #356 class of
/// guesswork).
pub fn open_failure_event(failure: &StreamFailure) -> (String, bool) {
    (failure.message.clone(), failure.retryable)
}

/// The resource path a device-flow wait probes between sleep slices (gh #566).
///
/// Nothing is ever stored here: every host answers "not found", and that answer
/// is not the point. The call is.
const WAIT_PROBE_RESOURCE: &str = "lca/device-flow-wait";

/// Sleep `span`, in 25 ms slices, touching the capability view between them
/// (gh #566).
///
/// The rule (`docs/capabilities.md`): a wait longer than the cancellation budget
/// polls in short slices. A WASM guest has no flag it could read - the only
/// thing that can interrupt it is a Wasmtime epoch trap, and a trap fires at a
/// **guest code point**, never inside a blocked host call. So each slice ends
/// with one real host import call, which is exactly the boundary the epoch
/// deadline can fire at; a single 5-15 s `thread::sleep` has no such boundary,
/// which is why a cancel during it waited out the whole interval.
///
/// Natively the same call reaches the capability engine the host's
/// `interrupt()` flagged. A host that aborts an in-flight call answers `Io`
/// ("request cancelled by the user"), so the wait reports the cancellation
/// instead of sleeping out the rest of the interval.
///
/// The flag is `ProviderCap::is_cancelled` - **defaulted to `false` on the
/// trait** (gh #566). It is not a host import: a guest keeps the default, because
/// no ABI import could carry a cancellation to it and a polled value could not
/// be acted on without the epoch trap that fires at its next host call. The
/// **native** view overrides it with the engine's real flag, and that is what
/// makes this wait work natively - the probe call beside it alone does *not*,
/// because `resource_read` is served from the extension's own tree and never
/// consults the flag (measured: the wait slept out its whole interval).
fn sleep_between_polls(
    cap: &dyn ProviderCap,
    span: std::time::Duration,
) -> Result<(), IdentityFailure> {
    use std::time::{Duration, Instant};
    const SLICE: Duration = Duration::from_millis(25);
    let deadline = Instant::now() + span;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        std::thread::sleep(remaining.min(SLICE));
        // Two observations, because the two delivery modes can see the cancel
        // in different places: the probe is a real host call (a guest's epoch
        // boundary), and the flag is what a native engine answers with. A host
        // call alone is not enough - `resource_read` is served from the
        // extension's own tree and never consults the flag, so a native wait
        // leaning on it alone sleeps out the whole interval (measured, gh #566).
        if cap.is_cancelled() {
            return Err(IdentityFailure(
                "device flow cancelled: the sign-in was interrupted".to_string(),
            ));
        }
        match cap.resource_read(WAIT_PROBE_RESOURCE) {
            // An abort: the host stopped the call, which is the cancel. Anything
            // else is this extension's own tree answering about a path it does
            // not define (not found, or a sandbox without the resources grant).
            Err(lca_protocol::CapabilityError::Io(reason))
            | Err(lca_protocol::CapabilityError::Timeout(reason)) => {
                return Err(IdentityFailure(format!("device flow cancelled: {reason}")));
            }
            _ => {}
        }
    }
}

/// Drive the poll loop to a token (pi's `pollOAuthDeviceCodeFlow`):
/// wait one interval first, floor intervals at one second, grow 5
/// seconds per `slow_down` (RFC 8628 section 3.5, the server's own
/// interval winning when valid), and expire on the authorization's
/// own clock. The waits are sliced, not one long sleep, so a cancel reaches
/// the login thread inside the cancellation budget (gh #566).
pub fn poll_device_code(
    cap: &dyn ProviderCap,
    authorization: &DeviceAuthorization,
    url: &str,
    pairs: &[(&str, &str)],
) -> Result<serde_json::Value, IdentityFailure> {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(authorization.expires_in_secs);
    let mut interval = Duration::from_secs(authorization.interval_secs.max(1));
    let mut slow_downs = 0u32;
    sleep_between_polls(
        cap,
        interval.min(deadline.saturating_duration_since(Instant::now())),
    )?;
    while Instant::now() < deadline {
        match poll_device_once(cap, url, pairs)? {
            DevicePollOutcome::Complete(json) => return Ok(json),
            DevicePollOutcome::Failed(message) => return Err(IdentityFailure(message)),
            DevicePollOutcome::SlowDown(server) => {
                slow_downs += 1;
                interval = match server {
                    Some(secs) => Duration::from_secs(secs.max(1)),
                    None => interval + Duration::from_secs(5),
                };
            }
            DevicePollOutcome::Pending => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        sleep_between_polls(cap, interval.min(remaining))?;
    }
    Err(IdentityFailure(if slow_downs > 0 {
        "device flow timed out after one or more slow_down responses; check the machine clock and try again"
            .to_string()
    } else {
        "device flow timed out".to_string()
    }))
}
