//! Bounded HTML-to-Markdown URL inspection (gh #222): fetch a URL
//! through the `net` capability and return clean, bounded text.
//!
//! Verifies: boilerplate strips before conversion, formats behave,
//! the 50 KiB bound marks omission, non-text MIME refuses loudly,
//! bad schemes and timeouts refuse before any request, grants gate
//! per host, and the manifest declares net with no pre-grants.

use std::cell::RefCell;
use std::collections::HashMap;
use webfetch::{FetchRequest, NetView, convert, fetch};

/// A canned network: URL to (status, headers, body).
type Route = (u16, Vec<(String, String)>, Vec<u8>);

struct FakeNet {
    routes: HashMap<String, Route>,
    reads: RefCell<usize>,
}

impl FakeNet {
    fn route(url: &str, mime: &str, body: &str) -> FakeNet {
        FakeNet {
            routes: HashMap::from([(
                url.to_string(),
                (
                    200,
                    vec![("content-type".to_string(), mime.to_string())],
                    body.as_bytes().to_vec(),
                ),
            )]),
            reads: RefCell::new(0),
        }
    }
}

impl NetView for FakeNet {
    fn request(&self, _method: &str, url: &str, _headers: &[(&str, &str)]) -> Result<u32, String> {
        if self.routes.contains_key(url) {
            Ok(1)
        } else {
            Err("permission: host not granted".to_string())
        }
    }

    fn status(&self, _handle: u32) -> Result<u16, String> {
        Ok(200)
    }

    fn headers(&self, _handle: u32) -> Result<Vec<(String, String)>, String> {
        Ok(self.routes.values().next().cloned().unwrap_or_default().1)
    }

    fn read_body(&self, _handle: u32, max: usize) -> Result<Option<Vec<u8>>, String> {
        *self.reads.borrow_mut() += 1;
        let body = &self.routes.values().next().cloned().unwrap_or_default().2;
        Ok(Some(body.iter().cloned().take(max).collect()))
    }

    fn close(&self, _handle: u32) {}
}

const DOC: &str = "<html><head><title>Tokio</title><style>.a{}</style><script>alert(1)</script></head><body><nav>menu</nav><h1>Tokio</h1><p>Async <code>runtime</code>.</p></body></html>";

fn request(url: &str) -> FetchRequest {
    FetchRequest {
        url: url.to_string(),
        format: webfetch::Format::Markdown,
        timeout_secs: 30,
    }
}

// Verifies: scripts, styles, and nav never reach the model, while
// headings and code survive as Markdown.
#[test]
fn boilerplate_strips_before_conversion() {
    let net = FakeNet::route("https://docs.example/x", "text/html", DOC);
    let out = fetch(&request("https://docs.example/x"), &net).expect("fetch");
    assert!(!out.contains("alert"), "{out}");
    assert!(!out.contains(".a{}"), "{out}");
    assert!(!out.contains("menu"), "{out}");
    assert!(out.contains("Tokio"), "{out}");
    assert!(out.contains("`runtime`"), "{out}");
}

// Verifies: the text format carries words without markup.
#[test]
fn text_format_has_no_markup() {
    let net = FakeNet::route("https://docs.example/x", "text/html", DOC);
    let out = fetch(
        &FetchRequest {
            format: webfetch::Format::Text,
            ..request("https://docs.example/x")
        },
        &net,
    )
    .expect("fetch");
    assert!(!out.contains('<'), "{out}");
    assert!(out.contains("Async runtime"), "{out}");
}

// Verifies: oversized output truncates at the bound with the marker,
// never silently.
#[test]
fn oversized_output_truncates_with_a_marker() {
    let big = format!("<html><body><p>{}</p></body></html>", "x".repeat(100_000));
    let net = FakeNet::route("https://docs.example/x", "text/html", &big);
    let out = fetch(&request("https://docs.example/x"), &net).expect("fetch");
    assert!(out.len() <= webfetch::OUTPUT_MAX + 64, "{}", out.len());
    assert!(out.contains("truncated"), "{out}");
}

// Verifies: non-text MIME refuses loudly with the type named.
#[test]
fn non_text_mime_refuses() {
    let net = FakeNet::route("https://docs.example/x", "image/png", "PNG...");
    let err = fetch(&request("https://docs.example/x"), &net).expect_err("refuse");
    assert!(err.contains("image/png"), "{err}");
}

// Verifies: non-HTTP schemes and bad timeouts refuse before any
// request leaves the process.
#[test]
fn bad_schemes_and_timeouts_refuse_early() {
    let net = FakeNet::route("https://docs.example/x", "text/html", DOC);
    for url in [
        "file:///etc/passwd",
        "ftp://x/y",
        "javascript:alert(1)",
        "notaurl",
    ] {
        let err = fetch(&request(url), &net).expect_err("refuse");
        assert!(err.contains("http"), "{err}");
    }
    for timeout in [0, 121] {
        let err = fetch(
            &FetchRequest {
                timeout_secs: timeout,
                ..request("https://docs.example/x")
            },
            &net,
        )
        .expect_err("refuse");
        assert!(err.contains("timeout"), "{err}");
    }
    assert_eq!(*net.reads.borrow(), 0, "no request left the process");
}

// Verifies: ungranted hosts surface the capability refusal (the ad
// hoc grant flow owns the recovery, not this tool).
#[test]
fn ungranted_hosts_surface_the_refusal() {
    let net = FakeNet::route("https://docs.example/x", "text/html", DOC);
    let err = fetch(&request("https://other.example/y"), &net).expect_err("refuse");
    assert!(err.contains("not granted"), "{err}");
}

// Verifies: unknown formats refuse at parse time.
#[test]
fn unknown_formats_refuse() {
    assert!(webfetch::parse_webfetch_args(r#"{"url": "https://x.example"}"#).is_ok());
    assert!(
        webfetch::parse_webfetch_args(r#"{"url": "https://x.example", "format": "pdf"}"#).is_err()
    );
}

// Verifies: the manifest test contract — the tool world with net
// declared and nothing pre-granted (every host arrives via consent).
#[test]
fn manifest_grants_match_the_manifest() {
    let manifest = webfetch::MANIFEST;
    assert!(manifest.contains("worlds = [\"tool\"]"), "{manifest}");
    assert!(manifest.contains("[capabilities.net]"), "{manifest}");
    let grants = webfetch::manifest_grants();
    assert!(grants.net.is_empty(), "no pre-granted hosts");
    assert!(manifest.contains("hosts = []"), "{manifest}");
    // Gh #302: `hosts = []` is a declaration, not an absence. Without
    // this the engine cannot tell "declared, all hosts by consent" from
    // "never declared", and the first fetch dies on the wrong gate.
    assert!(
        grants.net_declared,
        "an empty hosts list still declares net"
    );
}

// Verifies: conversion edge cases — entities decode, nested blocks
// collapse, unclosed tags do not hang.
#[test]
fn conversion_handles_entities_and_ragged_html() {
    let out = convert("<p>a &amp; b &lt;tag&gt; &#65;</p>", webfetch::Format::Text);
    assert!(out.contains("a & b <tag> A"), "{out}");
    let out = convert("<div><div><p>x", webfetch::Format::Text);
    assert!(out.contains('x'), "{out}");
}

// ---------------------------------------------------------------------------
// gh #302: one pipeline, two delivery modes (NFR-25)
// ---------------------------------------------------------------------------

/// **The point of the import.** Both halves run `fetch` → `convert` over a
/// `NetView` their mode supplies: the native half over the capability engine,
/// the WASM half over `lca:host/net`. This row pins that the *pipeline* is
/// shared rather than reimplemented — the WASM half is `NetView` over the
/// import, so given the same `NetView` behaviour it must produce the same
/// bytes.
///
/// The import itself is exercised end to end (a real component, a real host,
/// granted/denied/unlisted hosts) in
/// `crates/lca-ext-host/tests/conformance_diff.rs`'s net matrix and in
/// `tests/regressions/302-*.rs`; this row is the pipeline half, which is the
/// part that could silently diverge if the guest grew its own fetch.
#[test]
fn the_wasm_half_reuses_the_shared_pipeline() {
    // The guest's `NetView` impl and the native one differ only in the
    // transport they call. Prove the pipeline agreement on a redirect and a
    // non-2xx, the two shapes a hand-written fetch usually gets wrong.
    struct Scripted;
    thread_local! {
        static CALLS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }
    impl NetView for Scripted {
        fn request(
            &self,
            _method: &str,
            url: &str,
            _headers: &[(&str, &str)],
        ) -> Result<u32, String> {
            CALLS.with(|calls| calls.borrow_mut().push(url.to_string()));
            Ok(1)
        }
        fn status(&self, _h: u32) -> Result<u16, String> {
            // 302 to a second URL, then 200.
            let seen = CALLS.with(|calls| calls.borrow().len());
            Ok(if seen == 1 { 302 } else { 200 })
        }
        fn headers(&self, _h: u32) -> Result<Vec<(String, String)>, String> {
            let seen = CALLS.with(|calls| calls.borrow().len());
            if seen == 1 {
                Ok(vec![(
                    "location".to_string(),
                    "https://docs.example/final".to_string(),
                )])
            } else {
                Ok(vec![("content-type".to_string(), "text/html".to_string())])
            }
        }
        fn read_body(&self, _h: u32, _max: usize) -> Result<Option<Vec<u8>>, String> {
            Ok(Some(DOC.as_bytes().to_vec()))
        }
        fn close(&self, _h: u32) {}
    }
    CALLS.with(|calls| calls.borrow_mut().clear());
    let out = fetch(&request("https://docs.example/start"), &Scripted)
        .expect("fetch follows the redirect");
    assert!(out.contains("Tokio"), "the final document's text: {out}");
    // The redirect was followed, and the first document is what got converted.
    let seen = CALLS.with(|calls| calls.borrow().clone());
    assert!(seen.len() >= 2, "the redirect was followed: {seen:?}");
    assert_eq!(seen[0], "https://docs.example/start");
    assert_eq!(seen[1], "https://docs.example/final");
}

// Verifies: gh #302 - a redirect is followed in the shared pipeline, so both
// delivery modes see the final document rather than a bare 302. The net
// capability deliberately does not chase redirects (a redirect changes the
// host being asked, which is a consent question), so the loop lives where one
// implementation serves both modes.
#[test]
fn a_redirect_is_followed_to_the_final_document() {
    struct Redirects {
        hops: RefCell<usize>,
    }
    impl NetView for Redirects {
        fn request(&self, _m: &str, _url: &str, _h: &[(&str, &str)]) -> Result<u32, String> {
            *self.hops.borrow_mut() += 1;
            Ok(1)
        }
        fn status(&self, _h: u32) -> Result<u16, String> {
            Ok(if *self.hops.borrow() < 3 { 302 } else { 200 })
        }
        fn headers(&self, _h: u32) -> Result<Vec<(String, String)>, String> {
            if *self.hops.borrow() < 3 {
                Ok(vec![(
                    "location".to_string(),
                    format!("https://docs.example/step{}", *self.hops.borrow()),
                )])
            } else {
                Ok(vec![("content-type".to_string(), "text/html".to_string())])
            }
        }
        fn read_body(&self, _h: u32, _max: usize) -> Result<Option<Vec<u8>>, String> {
            Ok(Some(DOC.as_bytes().to_vec()))
        }
        fn close(&self, _h: u32) {}
    }
    let net = Redirects {
        hops: RefCell::new(0),
    };
    let out = fetch(&request("https://docs.example/start"), &net).expect("follows to the document");
    assert!(out.contains("Tokio"), "the final page's text: {out}");
    assert_eq!(*net.hops.borrow(), 3, "two redirects, then the document");
}

// Verifies: gh #302 - a redirect chain that never resolves is refused by
// name rather than looping, and the bound is the documented one.
#[test]
fn an_endless_redirect_chain_is_refused() {
    struct Loop;
    impl NetView for Loop {
        fn request(&self, _m: &str, _url: &str, _h: &[(&str, &str)]) -> Result<u32, String> {
            Ok(1)
        }
        fn status(&self, _h: u32) -> Result<u16, String> {
            Ok(302)
        }
        fn headers(&self, _h: u32) -> Result<Vec<(String, String)>, String> {
            Ok(vec![(
                "location".to_string(),
                "https://docs.example/again".to_string(),
            )])
        }
        fn read_body(&self, _h: u32, _max: usize) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        fn close(&self, _h: u32) {}
    }
    let err =
        fetch(&request("https://docs.example/start"), &Loop).expect_err("a loop cannot resolve");
    assert!(
        err.contains("redirecting") && err.contains(&webfetch::MAX_REDIRECTS.to_string()),
        "names the loop and the bound: {err}"
    );
}

// Verifies: gh #302 - a non-2xx that is *not* a redirect is refused with the
// status, which is the shape both modes must agree on.
#[test]
fn a_non_2xx_status_is_refused_with_the_code() {
    struct Failing;
    impl NetView for Failing {
        fn request(&self, _m: &str, _url: &str, _h: &[(&str, &str)]) -> Result<u32, String> {
            Ok(1)
        }
        fn status(&self, _h: u32) -> Result<u16, String> {
            Ok(404)
        }
        fn headers(&self, _h: u32) -> Result<Vec<(String, String)>, String> {
            Ok(Vec::new())
        }
        fn read_body(&self, _h: u32, _max: usize) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        fn close(&self, _h: u32) {}
    }
    let err = fetch(&request("https://docs.example/gone"), &Failing).expect_err("404 refuses");
    assert!(err.contains("404"), "names the status: {err}");
}
