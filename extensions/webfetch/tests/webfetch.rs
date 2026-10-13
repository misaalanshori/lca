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
