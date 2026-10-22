//! Unit rows for the inbound listener's pure parts: routing, the token's
//! constant-time comparison, the `Host` check, and the refusal vocabulary.
//!
//! The socket-level behaviour (bind, port fallback, auth over the wire,
//! shutdown) is pinned by `tests/regressions/363-inbound-http-capability.rs`,
//! which drives a real listener; these rows are the pieces that can be
//! asserted without one.

use super::serve_http::{DEFAULT_PORT, HttpResponse, PORT_ATTEMPTS, Refusal, SessionToken, route};

#[test]
fn a_mounted_path_is_split_into_session_extension_and_remainder() {
    let routed = route("/sess-1/ui-example/index.html").expect("routes");
    assert_eq!(routed.session, "sess-1");
    assert_eq!(routed.extension, "ui-example");
    assert_eq!(routed.rest, "/index.html");
}

#[test]
fn a_mount_root_routes_to_a_slash_remainder_so_index_handlers_agree() {
    // `/<session>/<ext>` with nothing after it is the same request a browser
    // makes for `/`: without this the extension's index handler would see an
    // empty path for one and `/` for the other.
    let routed = route("/sess/ext").expect("routes");
    assert_eq!(routed.rest, "/");
}

#[test]
fn a_path_naming_no_extension_does_not_route() {
    assert!(matches!(route("/"), Err(Refusal::NoRoute)));
    assert!(matches!(route("/only-a-session"), Err(Refusal::NoRoute)));
}

#[test]
fn a_dot_segment_in_the_mount_is_refused_rather_than_resolved() {
    // The host never resolves `..` here: a mount point is two names, and a
    // segment that is itself a traversal is an attempt against the namespace
    // boundary rather than a path for the extension to interpret.
    assert!(matches!(
        route("/../etc/passwd"),
        Err(Refusal::UnknownNamespace(_))
    ));
    assert!(matches!(
        route("/sess/../x"),
        Err(Refusal::UnknownNamespace(_))
    ));
}

#[test]
fn a_correct_token_matches_and_a_near_miss_does_not() {
    let token = SessionToken::from_known("abcdef0123456789");
    assert!(token.matches("abcdef0123456789"));
    assert!(!token.matches("abcdef012345678"));
    assert!(!token.matches("abcdef0123456789a"));
    assert!(!token.matches("abcdef0123456788"));
    assert!(!token.matches(""));
}

#[test]
fn a_generated_token_is_64_hex_characters_and_differs_each_time() {
    let first = SessionToken::generate().expect("random source");
    let second = SessionToken::generate().expect("random source");
    assert_eq!(first.expose().len(), 64);
    assert!(
        first.expose().chars().all(|c| c.is_ascii_hexdigit()),
        "got {}",
        first.expose()
    );
    assert_ne!(first.expose(), second.expose());
}

#[test]
fn a_token_never_prints_its_value_through_debug() {
    // The one secret this capability holds is displayed in the notice and
    // nowhere else. A `{:?}` in a diagnostic dump must not be a way to read it.
    let token = SessionToken::from_known("super-secret-value");
    let rendered = format!("{token:?}");
    assert!(!rendered.contains("super-secret-value"), "got {rendered}");
    assert!(rendered.contains("redacted"));
}

#[test]
fn the_default_port_range_is_the_issue_shape_and_bounded() {
    // The issue asks for 8080 with fallback to 8081...: a bounded range is
    // what makes "no port free" a real answer instead of an endless scan.
    assert_eq!(DEFAULT_PORT, 8080);
    assert_eq!(PORT_ATTEMPTS, 10);
}

#[test]
fn refusals_map_to_the_statuses_that_tell_a_client_the_truth() {
    assert_eq!(Refusal::Unauthorized.status(), 401);
    assert_eq!(Refusal::Rebinding("evil.example".into()).status(), 404);
    assert_eq!(Refusal::NoRoute.status(), 404);
    assert_eq!(Refusal::UnknownNamespace("s/e".into()).status(), 404);
    assert_eq!(Refusal::BodyTooLarge.status(), 413);
    assert_eq!(Refusal::Malformed("bad".into()).status(), 400);
    assert_eq!(Refusal::UpgradeNotSupported.status(), 501);
}

#[test]
fn an_unauthorized_refusal_does_not_distinguish_which_namespaces_exist() {
    // The body for every unknown-namespace case is the same 404 text as an
    // unauthenticated 401's cousin: the port must not be walkable.
    assert_eq!(
        Refusal::UnknownNamespace("s/e".into()).body(),
        Refusal::Rebinding("evil.example".into()).body()
    );
}

#[test]
fn a_scanner_produces_no_denial_record_but_a_rebinding_attempt_does() {
    // Journaling every token-less request would fill the denial log with
    // noise a user cannot act on; a foreign `Host` is a different fact.
    assert!(Refusal::Unauthorized.record().is_none());
    assert!(Refusal::NoRoute.record().is_none());
    let (_, parameter) = Refusal::Rebinding("evil.example:8080".into())
        .record()
        .expect("a rebinding attempt is worth recording");
    assert!(parameter.contains("evil.example:8080"), "got {parameter}");
}

#[test]
fn a_host_header_naming_another_origin_is_refused() {
    // The DNS-rebinding shape: the request arrived on loopback, so a
    // loopback-only bind has already passed, and the only thing left is the
    // header. Driven through the public routing predicate's own helper.
    let authority = "127.0.0.1:8080";
    assert!(super::serve_http::host_matches_for_test(
        "127.0.0.1:8080",
        authority
    ));
    assert!(super::serve_http::host_matches_for_test(
        "localhost:8080",
        authority
    ));
    assert!(super::serve_http::host_matches_for_test(
        "[::1]:8080",
        authority
    ));
    // A port mismatch is still a mismatch: the listener knows its own port.
    assert!(!super::serve_http::host_matches_for_test(
        "127.0.0.1:9999",
        authority
    ));
    assert!(!super::serve_http::host_matches_for_test(
        "evil.example:8080",
        authority
    ));
    assert!(!super::serve_http::host_matches_for_test(
        "evil.example",
        authority
    ));
}

#[test]
fn a_text_response_carries_a_content_type_a_browser_can_use() {
    let response = HttpResponse::text(404, "nope");
    assert_eq!(response.status, 404);
    assert!(
        response
            .headers
            .iter()
            .any(|(name, value)| name == "content-type" && value.contains("text/plain"))
    );
    assert_eq!(response.body, b"nope");
}
