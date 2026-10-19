//! Gh #315: a refresh rejected with RFC 6749 §5.2's `invalid_grant`
//! (HTTP **400**) left the stored tokens in place.
//!
//! The refresh path purged only on `401`. But an expired or revoked refresh
//! token is not an authentication failure at the transport layer — RFC 6749
//! §5.2 specifies `400 Bad Request` with `"error": "invalid_grant"` for it.
//! Because the tokens were never cleared, `access_token` purged and
//! reported… nothing: the next call found the same dead refresh token,
//! tried again, got the same 400, and the user was trapped in a permanent
//! refresh loop with credentials that could never work again. The only way
//! out was to know that `/logout` exists.
//!
//! The fix distinguishes the *error code* from the *status*: a 400 whose body
//! names `invalid_grant` is the RFC's "this token is dead, get a new one"
//! and purges; any other 400 (a wrong client, a malformed request) keeps the
//! credentials, because signing the user out over a server-side
//! misconfiguration would be worse than the error itself.
//!
//! Verifies: gh #315 acceptance 2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::collections::HashMap;
use std::sync::Mutex;

use lca_protocol::{CapabilityError, ProviderCap};
use lca_subscription::OAuthSpec;

/// A capability fake whose single response is `(status, body)`.
struct RefreshCap {
    status: u16,
    body: Vec<u8>,
    credentials: Mutex<HashMap<String, String>>,
    reads: Mutex<u32>,
}

impl RefreshCap {
    /// A cap holding an expired access token and a refresh token, so
    /// `access_token` takes the refresh path.
    fn expired(status: u16, body: &str) -> Self {
        let credentials = HashMap::from([
            ("access".to_string(), "stale-access".to_string()),
            ("refresh".to_string(), "dead-refresh".to_string()),
            // Expired ten seconds ago: inside the 60-second refresh window.
            ("expires".to_string(), "0".to_string()),
        ]);
        RefreshCap {
            status,
            body: body.as_bytes().to_vec(),
            credentials: Mutex::new(credentials),
            reads: Mutex::new(0),
        }
    }

    fn has(&self, key: &str) -> bool {
        self.credentials.lock().unwrap().contains_key(key)
    }

    fn get(&self, key: &str) -> Option<String> {
        self.credentials.lock().unwrap().get(key).cloned()
    }
}

impl ProviderCap for RefreshCap {
    fn net_request(
        &self,
        _method: &str,
        _url: &str,
        _headers: &[(&str, &str)],
        _body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        Ok(1)
    }

    fn net_response_status(&self, _handle: u32) -> Result<u16, CapabilityError> {
        Ok(self.status)
    }

    fn net_read_body(&self, _handle: u32, _max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut reads = self.reads.lock().unwrap();
        *reads += 1;
        Ok((*reads == 1).then(|| self.body.clone()))
    }

    fn net_close_response(&self, _handle: u32) -> Result<(), CapabilityError> {
        Ok(())
    }

    fn credentials_get(&self, key: &str) -> Option<String> {
        self.get(key)
    }

    fn credentials_set(&self, key: &str, value: &str) -> Result<(), CapabilityError> {
        self.credentials
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn credentials_delete(&self, key: &str) -> Result<(), CapabilityError> {
        self.credentials.lock().unwrap().remove(key);
        Ok(())
    }
}

fn spec() -> OAuthSpec {
    OAuthSpec {
        name: "codex",
        auth_endpoint: "https://auth.example.test/authorize",
        token_endpoint: "https://auth.example.test/token",
        revoke_endpoint: None,
        userinfo_endpoint: None,
        api_base: "https://api.example.test",
        client_id: "client-1",
        scope: "offline_access",
        extra_auth_params: &[],
        account: lca_subscription::AccountStrategy::JwtClaim {
            claim: "claim",
            field: "account",
        },
    }
}

/// RFC 6749 §5.2's canonical expired-refresh-token body.
const INVALID_GRANT: &str =
    r#"{"error":"invalid_grant","error_description":"The refresh token has expired"}"#;

// Verifies: gh #315 acceptance 2 - THE defect. A 400 whose body says
// `invalid_grant` purges, so the next call says "run /login" instead of
// retrying a token that can never work.
#[test]
fn a_400_invalid_grant_purges_the_dead_tokens() {
    let cap = RefreshCap::expired(400, INVALID_GRANT);
    let err =
        lca_subscription::access_token(&cap, &spec()).expect_err("a rejected refresh is an error");
    assert!(!cap.has("refresh"), "the dead refresh token is gone");
    assert!(!cap.has("access"), "the stale access token is gone");
    assert!(!cap.has("expires"), "the stale expiry is gone");
    // And the message points at the way out, per the error-copy policy.
    let text = err.0.to_lowercase();
    assert!(
        text.contains("login") || text.contains("sign"),
        "the failure should tell the user to sign in again, got: {}",
        err.0
    );
}

// Verifies: gh #315 acceptance 2 - a 400 with a DIFFERENT error keeps the
// credentials. A wrong client id, a malformed request, or a server-side
// misconfiguration must not sign the user out: the tokens are still good
// and the next attempt may succeed once the server is fixed.
#[test]
fn a_400_with_another_error_keeps_the_credentials() {
    let cap = RefreshCap::expired(
        400,
        r#"{"error":"invalid_client","error_description":"unknown client"}"#,
    );
    let _ = lca_subscription::access_token(&cap, &spec()).expect_err("still an error");
    assert!(
        cap.has("refresh"),
        "a wrong-client error is not our tokens' fault"
    );
    assert!(cap.has("access"));
    assert!(cap.has("expires"));
}

// Verifies: gh #315 acceptance 2 - 401 is unchanged: it still purges (the
// pre-existing behavior, which was right).
#[test]
fn a_401_still_purges() {
    let cap = RefreshCap::expired(401, r#"{"error":"invalid_token"}"#);
    let _ = lca_subscription::access_token(&cap, &spec()).expect_err("error");
    assert!(!cap.has("refresh"), "401 purges as before");
    assert!(!cap.has("access"));
}

// Verifies: gh #315 - a 200 that carries a token refreshes and KEEPS
// everything (the happy path must not have been disturbed by the fix).
#[test]
fn a_200_stores_the_new_tokens_and_purges_nothing() {
    let cap = RefreshCap::expired(
        200,
        r#"{"access_token":"fresh-access","refresh_token":"fresh-refresh","expires_in":3600}"#,
    );
    let token = lca_subscription::access_token(&cap, &spec()).expect("refreshed");
    assert_eq!(token, "fresh-access");
    assert_eq!(cap.get("access").as_deref(), Some("fresh-access"));
    assert_eq!(cap.get("refresh").as_deref(), Some("fresh-refresh"));
    assert!(cap.has("expires"));
}

// Verifies: gh #315 - a 500 does not purge: a transient server failure says
// nothing about the token's validity, and purging would force a re-login
// over an outage.
#[test]
fn a_500_does_not_purge() {
    let cap = RefreshCap::expired(500, "upstream exploded");
    let _ = lca_subscription::access_token(&cap, &spec()).expect_err("error");
    assert!(cap.has("refresh"), "an outage is not a credential verdict");
    assert!(cap.has("access"));
}

// Verifies: gh #315 - the match is on the parsed `error` FIELD, not a
// substring of the body. A body that merely mentions `invalid_grant` in a
// description, under a different error code, must not purge.
#[test]
fn the_match_reads_the_error_field_not_the_text() {
    let cap = RefreshCap::expired(
        400,
        r#"{"error":"server_error","error_description":"this is not invalid_grant exactly"}"#,
    );
    let _ = lca_subscription::access_token(&cap, &spec()).expect_err("error");
    assert!(
        cap.has("refresh"),
        "the error CODE decides; a substring match would purge here"
    );
}

// Verifies: gh #315 - a 400 with a non-JSON body keeps the credentials: we
// cannot tell what it means, so we do not destroy the user's tokens on a
// guess. (Refusing to purge is the conservative direction: the worst case is
// one more failed call, versus a forced re-login.)
#[test]
fn an_unparseable_400_body_keeps_the_credentials() {
    let cap = RefreshCap::expired(400, "<html>bad gateway</html>");
    let _ = lca_subscription::access_token(&cap, &spec()).expect_err("error");
    assert!(cap.has("refresh"), "an unreadable body is not a verdict");
}

// Verifies: gh #315 - the RFC's other dead-refresh spelling. Some servers
// answer `invalid_request` for a malformed refresh token, but the RFC's own
// code for an unusable grant is `invalid_grant`; the fix is deliberately
// narrow so an unrelated `invalid_request` cannot sign a user out.
#[test]
fn a_400_invalid_request_keeps_the_credentials() {
    let cap = RefreshCap::expired(400, r#"{"error":"invalid_request"}"#);
    let _ = lca_subscription::access_token(&cap, &spec()).expect_err("error");
    assert!(
        cap.has("refresh"),
        "only invalid_grant is a dead-token verdict"
    );
}
