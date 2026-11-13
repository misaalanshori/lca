//! The loopback login and logout (gh #567: split out of `lib.rs` by OAuth
//! concern).

use lca_protocol::{IdentityOutcome, OauthCap, ProviderCap};

use crate::IdentityFailure;
use crate::crypto::{base64url, percent, random_bytes};
use crate::json_error_message;
use crate::spec::{AccountStrategy, OAuthSpec, endpoint, stored};
use crate::token::{account_from_jwt, account_from_userinfo};
use crate::transport::post_form;

/// `login`: the loopback OAuth PKCE flow every subscription gateway
/// shares — bind, authorize, await the callback (loopback or pasted),
/// exchange, learn the account, store. A login always runs the flow,
/// even with tokens stored: the invocation itself is the overwrite
/// prompt (gh #179's deadlock, ported forward).
pub fn run_login(
    cap: &dyn ProviderCap,
    oauth: &dyn OauthCap,
    spec: &OAuthSpec,
) -> Result<IdentityOutcome, IdentityFailure> {
    let verifier = base64url(&random_bytes(32)?);
    let challenge = {
        use sha2::Digest as _;
        base64url(&sha2::Sha256::digest(verifier.as_bytes()))
    };
    let state = base64url(&random_bytes(32)?);
    let (redirect, flow) = oauth
        .oauth_begin("/callback")
        .map_err(|err| IdentityFailure(format!("cannot start the loopback flow: {err}")))?;
    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&\
         code_challenge_method=S256&state={}",
        endpoint(cap, "auth_endpoint", spec.auth_endpoint),
        percent(spec.client_id),
        percent(&redirect),
        percent(spec.scope),
        challenge,
        state,
    );
    for (key, value) in spec.extra_auth_params {
        url.push_str(&format!("&{}={}", percent(key), percent(value)));
    }
    // Best effort like every provider: a machine with no browser
    // launcher still completes through the printed URL and a paste.
    let _ = oauth.oauth_open(&url);
    let callback = oauth.oauth_await(flow);
    let _ = oauth.oauth_end(flow);
    let callback = callback.map_err(|err| IdentityFailure(err.to_string()))?;
    let get = |key: &str| {
        callback
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    if get("state") != state {
        return Err(IdentityFailure(
            "OAuth state mismatch; the callback did not come from this flow".to_string(),
        ));
    }
    let code = get("code");
    if code.is_empty() {
        let problem = get("error_description");
        let problem = if problem.is_empty() {
            get("error")
        } else {
            problem
        };
        return Err(IdentityFailure(format!(
            "the authorization server returned no code: {problem}"
        )));
    }
    let token_url = endpoint(cap, "token_endpoint", spec.token_endpoint);
    let (status, text) = post_form(
        cap,
        &token_url,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", spec.client_id),
            ("code", &code),
            ("code_verifier", &verifier),
            ("redirect_uri", &redirect),
        ],
    )?;
    if !(200..300).contains(&status) {
        return Err(IdentityFailure(format!(
            "token exchange failed: {}",
            json_error_message(&text)
        )));
    }
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| IdentityFailure(format!("token reply: {err}")))?;
    let access = json
        .get("access_token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| IdentityFailure("no access token in the reply".to_string()))?
        .to_string();
    let refresh = json
        .get("refresh_token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            IdentityFailure(
                "no refresh token received; re-run login and allow offline access".to_string(),
            )
        })?
        .to_string();
    let expires_in = json
        .get("expires_in")
        .and_then(|value| value.as_u64())
        .unwrap_or(3600);
    let account = match spec.account {
        AccountStrategy::JwtClaim { claim, field } => account_from_jwt(&access, claim, field)
            .ok_or_else(|| IdentityFailure("the access token carries no account".to_string()))?,
        AccountStrategy::Userinfo { field } => {
            let url = spec
                .userinfo_endpoint
                .ok_or_else(|| IdentityFailure("no userinfo endpoint configured".to_string()))?;
            let url = endpoint(cap, "userinfo_endpoint", url);
            account_from_userinfo(cap, &url, &access, field)?
        }
    };
    for (key, value) in [
        ("access", access.as_str()),
        ("refresh", refresh.as_str()),
        (
            "expires",
            (crate::now_epoch() + expires_in).to_string().as_str(),
        ),
        ("account_id", account.as_str()),
    ] {
        cap.credentials_set(key, value)
            .map_err(|err| IdentityFailure(format!("cannot store {key}: {err}")))?;
    }
    Ok(IdentityOutcome::Ok)
}

/// `logout`: revoke server-side where the gateway supports it, then
/// clear the namespace. A revoke that fails still ends cleared — the
/// local state is what the user asked about.
pub fn run_logout(cap: &dyn ProviderCap, spec: &OAuthSpec) -> IdentityOutcome {
    if let Some(revoke) = spec.revoke_endpoint {
        // RFC 7009 shape (the refresh token, like fx revokes).
        let refresh = stored(cap, "refresh");
        if !refresh.is_empty() {
            let url = endpoint(cap, "revoke_endpoint", revoke);
            let _ = post_form(
                cap,
                &url,
                &[("token", &refresh), ("client_id", spec.client_id)],
            );
        }
    }
    for key in ["access", "refresh", "expires", "account_id"] {
        if cap.credentials_delete(key).is_err() {
            // A missing key is already the desired end state.
        }
    }
    IdentityOutcome::Ok
}
