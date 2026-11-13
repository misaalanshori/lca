//! The bounded request/response transport these flows use, and the clock
//! (gh #567: split out of `lib.rs` by OAuth concern).

use lca_protocol::ProviderCap;

use crate::IdentityFailure;
use crate::crypto::percent;

/// POST a form and read the whole response (the token calls are small
/// and request/response, unlike the stream).
///
/// Delegates rather than repeating the transport (gh #339 finding 2): this was a
/// line-for-line copy of [`post_form_accept`] minus the `accept` header, and two
/// copies of a bounded read, a status call and a close is three places for the
/// 1 MiB cap or the close to drift out of.
pub(crate) fn post_form(
    cap: &dyn ProviderCap,
    url: &str,
    pairs: &[(&str, &str)],
) -> Result<(u16, String), IdentityFailure> {
    post_form_with_accept(cap, url, pairs, None)
}

/// POST one JSON object over capabilities and read the bounded
/// reply (gh #183, gh #185): the Anthropic and OpenRouter token
/// exchanges speak JSON where the older gateways speak form. Same
/// one-megabyte bound as the form POST below; the caller names the status.
pub fn post_json(
    cap: &dyn ProviderCap,
    url: &str,
    body: &serde_json::Value,
) -> Result<(u16, String), IdentityFailure> {
    post_json_with_headers(cap, url, &[], body)
}

/// POST one JSON object with extra headers (gh #186: Meta's mint
/// authenticates by header, not by body).
pub fn post_json_with_headers(
    cap: &dyn ProviderCap,
    url: &str,
    extra: &[(&str, &str)],
    body: &serde_json::Value,
) -> Result<(u16, String), IdentityFailure> {
    let bytes = serde_json::to_vec(body)
        .map_err(|err| IdentityFailure(format!("cannot build request: {err}")))?;
    let mut headers = vec![
        ("content-type", "application/json"),
        ("accept", "application/json"),
    ];
    headers.extend_from_slice(extra);
    let handle = cap
        .net_request("POST", url, &headers, Some(&bytes))
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let status = cap
        .net_response_status(handle)
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let mut text = Vec::new();
    while let Some(chunk) = cap
        .net_read_body(handle, 64 * 1024)
        .map_err(|err| IdentityFailure(err.to_string()))?
    {
        text.extend_from_slice(&chunk);
        if text.len() > 1024 * 1024 {
            break;
        }
    }
    let _ = cap.net_close_response(handle);
    Ok((status, String::from_utf8_lossy(&text).into_owned()))
}

/// POST form fields and read the bounded reply with an `Accept` header.
/// Public for the form-speaking token endpoints (gh #187's refresh).
pub fn post_form_accept(
    cap: &dyn ProviderCap,
    url: &str,
    pairs: &[(&str, &str)],
    accept: &str,
) -> Result<(u16, String), IdentityFailure> {
    post_form_with_accept(cap, url, pairs, Some(accept))
}

/// The one form POST (gh #339 finding 2): `accept` is `None` for the callers that
/// want the endpoint's own default, `Some` for the device endpoints that answer
/// url-encoded without it.
pub(crate) fn post_form_with_accept(
    cap: &dyn ProviderCap,
    url: &str,
    pairs: &[(&str, &str)],
    accept: Option<&str>,
) -> Result<(u16, String), IdentityFailure> {
    let body: String = pairs
        .iter()
        .map(|(key, value)| format!("{}={}", percent(key), percent(value)))
        .collect::<Vec<_>>()
        .join("&");
    // Built by hand rather than as a fixed-length array: the `accept` pair is the
    // only difference between the two callers, and taking it as an option is what
    // lets one implementation serve both.
    let mut headers = vec![("content-type", "application/x-www-form-urlencoded")];
    if let Some(accept) = accept {
        headers.push(("accept", accept));
    }
    let handle = cap
        .net_request("POST", url, &headers, Some(body.as_bytes()))
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let status = cap
        .net_response_status(handle)
        .map_err(|err| IdentityFailure(err.to_string()))?;
    let mut text = Vec::new();
    while let Some(chunk) = cap
        .net_read_body(handle, 64 * 1024)
        .map_err(|err| IdentityFailure(err.to_string()))?
    {
        text.extend_from_slice(&chunk);
        if text.len() > 1024 * 1024 {
            break;
        }
    }
    let _ = cap.net_close_response(handle);
    Ok((status, String::from_utf8_lossy(&text).into_owned()))
}

/// Seconds since the Unix epoch (bounds, labels, expiries).
pub fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}
