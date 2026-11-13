//! Credential lifecycle: reading, refreshing, purging, and the account id
//! (gh #567: split out of `lib.rs` by OAuth concern).

use lca_protocol::ProviderCap;

use crate::IdentityFailure;
use crate::crypto::base64url_decode;
use crate::json_error_message;
use crate::spec::{OAuthSpec, endpoint, stored};
use crate::transport::post_form;

/// Clear the stored tokens after a rejection: a revoked or expired
/// token must not short-circuit the next login or poison the next
/// call. Best effort — the caller's error carries the news.
pub fn purge_tokens(cap: &dyn ProviderCap) {
    for key in ["access", "refresh", "expires"] {
        let _ = cap.credentials_delete(key);
    }
}

/// Whether a refresh rejection means the grant itself is dead
/// (gh #315): RFC 6749 §5.2's `invalid_grant`.
///
/// Read from the parsed `error` FIELD, never from the body text: the
/// description of an unrelated failure can contain the same words, and
/// signing a user out over a server-side error is worse than the error.
/// An unparseable body is not a verdict either - the conservative
/// direction is one more failed call, not a forced re-login.
pub(crate) fn refresh_grant_is_dead(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|json| {
            json.get("error")
                .and_then(|error| error.as_str())
                .map(str::to_string)
        })
        .is_some_and(|error| error == "invalid_grant")
}

/// The access token, refreshed first when it is within a minute of
/// expiry and a refresh token exists (FR-PROV-5). A rejected refresh
/// purges, so the next call re-authenticates instead of looping.
pub fn access_token(cap: &dyn ProviderCap, spec: &OAuthSpec) -> Result<String, IdentityFailure> {
    let access = stored(cap, "access");
    if access.is_empty() {
        return Err(IdentityFailure(format!(
            "no {} login yet; run /login {}",
            spec.name, spec.name
        )));
    }
    let expires: u64 = stored(cap, "expires").parse().unwrap_or(0);
    if crate::now_epoch() + 60 < expires {
        return Ok(access);
    }
    let refresh = stored(cap, "refresh");
    if refresh.is_empty() {
        return Err(IdentityFailure(format!(
            "the stored {} token is expired and there is no refresh token; run /login {}",
            spec.name, spec.name
        )));
    }
    let url = endpoint(cap, "token_endpoint", spec.token_endpoint);
    let (status, text) = post_form(
        cap,
        &url,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", spec.client_id),
            ("refresh_token", &refresh),
        ],
    )?;
    if !(200..300).contains(&status) {
        // Gh #315: an expired or revoked refresh token is RFC 6749
        // §5.2's **400 `invalid_grant`**, not a 401 - the transport
        // authenticated fine and the request was well formed; the grant
        // itself is dead. Purging only on 401 left the dead token in
        // place, so every later turn retried it, got the same 400, and
        // the user was trapped in a refresh loop whose only exit was
        // knowing that `/logout` exists.
        //
        // The verdict reads the parsed `error` field, never a substring
        // of the body: a server-side error that merely mentions the
        // words must not sign the user out. Every other 4xx keeps the
        // credentials, because a wrong client id or a malformed request
        // says nothing about whether the token is still good, and a
        // spurious purge costs the user a full re-login.
        if status == 401 || (status == 400 && refresh_grant_is_dead(&text)) {
            purge_tokens(cap);
            return Err(IdentityFailure(format!(
                "the {} session has expired or was revoked; run /login {} to sign in again",
                spec.name, spec.name
            )));
        }
        return Err(IdentityFailure(format!(
            "token refresh failed: {}",
            json_error_message(&text)
        )));
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| IdentityFailure(format!("refresh: {err}")))?;
    let access = json
        .get("access_token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| IdentityFailure("refresh returned no access token".to_string()))?
        .to_string();
    let expires_in = json
        .get("expires_in")
        .and_then(|value| value.as_u64())
        .unwrap_or(3600);
    cap.credentials_set("access", &access)
        .map_err(|err| IdentityFailure(format!("cannot store the token: {err}")))?;
    cap.credentials_set("expires", &(crate::now_epoch() + expires_in).to_string())
        .map_err(|err| IdentityFailure(format!("cannot store the expiry: {err}")))?;
    if let Some(rotated) = json.get("refresh_token").and_then(|value| value.as_str()) {
        cap.credentials_set("refresh", rotated)
            .map_err(|err| IdentityFailure(format!("cannot store the refresh token: {err}")))?;
    }
    Ok(access)
}

/// Decode the account id from the access token's JWT payload.
/// Exported for live smokes, which seed a scratch namespace from an
/// operator-provided token.
pub fn account_from_jwt(token: &str, claim: &str, field: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let json: serde_json::Value = serde_json::from_slice(&base64url_decode(payload)?).ok()?;
    json.get(claim)?.get(field)?.as_str().map(str::to_string)
}

/// GET the userinfo endpoint with the token and read one field.
pub(crate) fn account_from_userinfo(
    cap: &dyn ProviderCap,
    url: &str,
    token: &str,
    field: &str,
) -> Result<String, IdentityFailure> {
    let bearer = format!("Bearer {token}");
    let handle = cap
        .net_request("GET", url, &[("authorization", bearer.as_str())], None)
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
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "account lookup failed: {}",
            json_error_message(&String::from_utf8_lossy(&text))
        )));
    }
    serde_json::from_slice::<serde_json::Value>(&text)
        .ok()
        .and_then(|json| json.get(field)?.as_str().map(str::to_string))
        .ok_or_else(|| IdentityFailure("account lookup returned no account".to_string()))
}
