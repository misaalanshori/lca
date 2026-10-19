//! Gh #315: a refresh rejected with RFC 6749 §5.2's `invalid_grant` left the
//! stored tokens in place, trapping the user in a permanent refresh loop.
//!
//! The kit purged credentials only on HTTP 401. But 401 is the wrong status
//! for this failure: an expired or revoked **refresh token** is a well-formed
//! request whose *grant* is dead, and RFC 6749 §5.2 answers
//! `400 Bad Request` with `{"error": "invalid_grant"}`. So the tokens
//! survived, `access_token` retried the same dead refresh token, got the same
//! 400, and every turn failed identically - with no path out short of
//! knowing that `/logout` exists.
//!
//! The fix reads the parsed `error` field: `401` still purges (unchanged),
//! `400` + `invalid_grant` now purges too, and every other status keeps the
//! credentials, because a wrong client id or a server outage says nothing
//! about whether the token is still good - and a spurious purge costs the
//! user a full re-login.
//!
//! Verifies: gh #315 acceptance 2.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

const KIT: &str = include_str!("../../crates/lca-subscription/src/lib.rs");

// Verifies: gh #315 acceptance 2 - the status check includes 400 alongside
// 401. This is the direct assertion on the defect: the old code tested
// `status == 401` alone.
#[test]
fn the_rejection_check_covers_the_rfc_400_case() {
    assert!(
        KIT.contains("status == 401 || (status == 400 && refresh_grant_is_dead(&text))"),
        "the purge condition must cover 401 OR 400-invalid_grant; 401 alone is the bug"
    );
}

// Verifies: gh #315 acceptance 2 - the verdict reads the parsed `error`
// FIELD. A substring search over the body would purge on any server whose
// description happens to contain the words, which is the opposite failure
// (signing a user out over an unrelated error).
#[test]
fn the_verdict_parses_the_error_field() {
    assert!(
        KIT.contains("fn refresh_grant_is_dead"),
        "the predicate is named so its contract is greppable"
    );
    let body = KIT
        .split("fn refresh_grant_is_dead")
        .nth(1)
        .expect("the predicate body");
    assert!(
        body.contains("serde_json::from_str"),
        "the body is parsed as JSON, never substring-matched"
    );
    assert!(
        body.contains(".get(\"error\")"),
        "the `error` field is what decides, not the description"
    );
    assert!(
        !body.contains("contains(\"invalid_grant\")"),
        "a substring match would purge on an unrelated error that mentions the words"
    );
}

// Verifies: gh #315 acceptance 2 - a purged session reports an honest,
// actionable outcome rather than a bare status. The user needs to know to
// sign in again, and the error-copy policy says what to DO.
#[test]
fn a_purged_session_says_to_log_in_again() {
    assert!(
        KIT.contains("run /login {}"),
        "the dead-grant failure must name the way back"
    );
    // And it is the *purge* arm that says it, not the generic arm: locate
    // the purge call and require the actionable copy in the same statement
    // chain, with whitespace normalized (rustfmt reflows this block).
    let flat = KIT.split_whitespace().collect::<Vec<_>>().join(" ");
    let arm_start = flat
        .find("purge_tokens(cap);")
        .expect("the purge call exists");
    let arm = &flat[arm_start..];
    assert!(
        arm.starts_with("purge_tokens(cap); return Err(IdentityFailure(")
            && arm[..arm.len().min(300)].contains("run /login {} to sign in again"),
        "the purge arm must carry the sign-in-again copy, got: {}",
        &arm[..arm.len().min(300)]
    );
}
