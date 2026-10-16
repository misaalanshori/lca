//! GitHub #343: `redact_url` and `redact_json` matched keys by exact
//! equality against a six-name list, so standard OAuth keys —
//! `access_token`, `id_token`, and the callback `code` — passed through
//! in cleartext to `debug-wire.log`. The list is replaced by one
//! normalized matcher (lowercase, `_`/`-` stripped, exact + suffix).
//!
//! Verifies: the public redaction API never lets a fake OAuth credential
//! reach the produced log bytes, in JSON payloads, callback URLs, and
//! cookies, while usage counters stay visible.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::io::Write;

use lca_debug::logfiles::{DebugPaths, RotatingWriter};
use lca_debug::redact::{redact_header, redact_json, redact_url};

const TOKEN: &str = "FAKESECRET-343-ACCESS";
const ID_TOKEN: &str = "FAKESECRET-343-IDT";
const CODE: &str = "FAKESECRET-343-CODE";

// Verifies: gh #343 - an OAuth token-exchange payload written through the
// real rotating writer never carries the credentials unmasked.
#[test]
fn an_oauth_payload_never_reaches_the_wire_log_unmasked() {
    let root = lca_testkit::scratch_path("regression-343-redact");
    let _ = std::fs::remove_dir_all(&root);
    let paths = DebugPaths::in_dir(&root);
    let mut writer = RotatingWriter::open(&paths.wire).expect("open");

    let payload = format!(
        r#"{{"access_token":"{TOKEN}","id_token":"{ID_TOKEN}","refresh_token":"FAKESECRET-343-REFRESH","client_secret":"FAKESECRET-343-SECRET","token_type":"Bearer","expires_in":3600}}"#
    );
    let line = format!(
        "{}\n{}\n{}\n",
        redact_json(&payload),
        redact_url(&format!(
            "http://127.0.0.1:8080/callback?code={CODE}&state=xyz"
        )),
        redact_header("cookie", &format!("sessionId={TOKEN}; theme=dark")),
    );
    writer.write_all(line.as_bytes()).expect("write");
    writer.flush().expect("flush");

    let logged = std::fs::read_to_string(&paths.wire).expect("read log");
    for secret in [
        TOKEN,
        ID_TOKEN,
        CODE,
        "FAKESECRET-343-REFRESH",
        "FAKESECRET-343-SECRET",
    ] {
        assert!(
            !logged.contains(secret),
            "{secret} reached the wire log:\n{logged}"
        );
    }
    // Readability survives: the schema keys and usage counters are present.
    assert!(logged.contains("access_token"), "the key name survives");
    assert!(logged.contains("token_type"), "token_type stays visible");
    assert!(logged.contains("expires_in"), "the schema survives");
    assert!(
        logged.contains("state=xyz"),
        "a non-credential param survives"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #343 fix-up - a secret nested under a sensitive CONTAINER
// never reaches the produced log, even when the leaf name is innocuous
// (`token.value`, `credentials.AccessKeyId`, `auth.bearer`, `session.id`).
// The prior recursion preserved those keys but let the leaves through.
#[test]
fn a_secret_under_a_sensitive_container_never_reaches_the_log_unmasked() {
    let root = lca_testkit::scratch_path("regression-343-container");
    let _ = std::fs::remove_dir_all(&root);
    let paths = DebugPaths::in_dir(&root);
    let mut writer = RotatingWriter::open(&paths.wire).expect("open");

    let payload = r#"{"token":{"value":"FAKESECRET-CT-TVAL"},"credentials":{"AccessKeyId":"AKIA-FAKECT","SecretAccessKey":"FAKESECRET-CT-AK"},"auth":{"bearer":"eyJhbGciOi.FAKECT"},"session":{"id":"sess-FAKECT"},"meta":{"model":"m"}}"#;
    let line = format!("{}\n", redact_json(payload));
    writer.write_all(line.as_bytes()).expect("write");
    writer.flush().expect("flush");

    let logged = std::fs::read_to_string(&paths.wire).expect("read log");
    for secret in [
        "FAKESECRET-CT-TVAL",
        "AKIA-FAKECT",
        "FAKESECRET-CT-AK",
        "eyJhbGciOi.FAKECT",
        "sess-FAKECT",
    ] {
        assert!(
            !logged.contains(secret),
            "{secret} reached the wire log:\n{logged}"
        );
    }
    // Schema and non-sensitive siblings survive.
    for key in ["token", "value", "AccessKeyId", "bearer", "id"] {
        assert!(logged.contains(key), "the key {key} survives");
    }
    assert!(
        logged.contains("\"model\":\"m\""),
        "an innocent leaf survives"
    );
    let _ = std::fs::remove_dir_all(&root);
}
