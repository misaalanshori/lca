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

/// A canned network that models **both ways a body can end** (gh #351 finding 1).
///
/// It used to answer `Some(body)` on every read, for ever: no read ever
/// returned `None`, so the fetch loop only ever left by hitting its 1 MiB input
/// cap, and the path a *complete* fetch takes had never run under test. Worse,
/// the body was re-delivered each time, so a 166-byte document was appended to
/// itself **6,317 times** and came back as a 51 KiB "page" ending in the
/// truncation marker - while every row that only asserted "the text is present"
/// passed, which is the textbook test that passes for the wrong reason.
///
/// So the fake now has a cursor and a chunk size: `read_body` hands out the
/// body in pieces and then answers `Ok(None)`, which is what the real transport
/// does at end of stream (`Capabilities::net_read_body` returns `Ok(None)`
/// exactly when it has no bytes left). A complete body therefore costs
/// `ceil(len/chunk) + 1` reads - the data, then the `None` - and that count is
/// what the rows assert, because the bytes alone cannot tell the two endings
/// apart.
///
/// `request` resets the cursor: a new request is a new body stream, which is
/// also what makes the redirect rows honest (each hop reads its own response
/// from the start).
struct FakeNet {
    routes: HashMap<String, Route>,
    /// Bytes handed out per read. `route` sets it to the whole body, so the
    /// default is "one data read, then EOF".
    chunk: usize,
    cursor: RefCell<usize>,
    reads: RefCell<usize>,
    /// A transport that answers `Some(Vec::new())` over and over. Out of
    /// contract (`None` means EOF), and reachable only from a broken adapter -
    /// but the read loop must end on it rather than spin, so a row pins it.
    ///
    /// It gives up after 100 answers with an error rather than answering for
    /// ever: a loop that does *not* end then fails with a readable message
    /// instead of hanging until the harness timeout, which is the difference
    /// between a red row and a lost CI job. (Verified both ways: with the
    /// production loop's empty-chunk guard removed, this fails in 19 ms saying
    /// the loop spun; without the cap it took the full 120 s harness timeout.)
    empty_forever: bool,
}

impl FakeNet {
    /// The whole body in a single read, then EOF.
    fn route(url: &str, mime: &str, body: &str) -> FakeNet {
        FakeNet {
            chunk: body.len().max(1),
            routes: HashMap::from([(
                url.to_string(),
                (
                    200,
                    vec![("content-type".to_string(), mime.to_string())],
                    body.as_bytes().to_vec(),
                ),
            )]),
            cursor: RefCell::new(0),
            reads: RefCell::new(0),
            empty_forever: false,
        }
    }

    /// The same route, delivered `chunk` bytes per read: the transport a real
    /// connection gives you, and the only way to count reads meaningfully.
    fn in_chunks(url: &str, mime: &str, body: &str, chunk: usize) -> FakeNet {
        FakeNet {
            chunk: chunk.max(1),
            ..FakeNet::route(url, mime, body)
        }
    }

    /// A broken adapter: `Some(empty)` on every read, never `None`.
    fn empty_forever(url: &str, mime: &str) -> FakeNet {
        FakeNet {
            empty_forever: true,
            ..FakeNet::route(url, mime, "")
        }
    }

    fn reads(&self) -> usize {
        *self.reads.borrow()
    }
}

impl NetView for FakeNet {
    fn request(&self, _method: &str, url: &str, _headers: &[(&str, &str)]) -> Result<u32, String> {
        if self.routes.contains_key(url) {
            // A request opens a fresh body stream.
            *self.cursor.borrow_mut() = 0;
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
        if self.empty_forever {
            let read = *self.reads.borrow();
            if read > 100 {
                return Err(format!(
                    "the read loop called back {read} times without a byte arriving - \
                     it is spinning on empty chunks instead of ending"
                ));
            }
            return Ok(Some(Vec::new()));
        }
        let body = &self.routes.values().next().cloned().unwrap_or_default().2;
        let mut cursor = self.cursor.borrow_mut();
        if *cursor >= body.len() {
            // **End of stream.** This is the answer the old fake never gave.
            return Ok(None);
        }
        let take = self.chunk.min(max).min(body.len() - *cursor);
        let chunk = body[*cursor..*cursor + take].to_vec();
        *cursor += take;
        Ok(Some(chunk))
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

// ---------------------------------------------------------------------------
// gh #351 finding 1: the two ways a body can end
// ---------------------------------------------------------------------------

// Verifies: gh #351 finding 1 - a **complete** body is read to EOF (`Ok(None)`),
// and the result is byte-complete with **no truncation marker**.
//
// This is the row that could not be written before the fake could say `None`:
// every row in this file exited through the input cap, so "a fetch that
// succeeds" was never once exercised. It is deliberately a small document, so
// the input cap cannot be what ended the read.
#[test]
fn a_complete_body_is_read_to_eof_and_is_not_truncated() {
    let net = FakeNet::route("https://docs.example/x", "text/html", DOC);
    let out = fetch(&request("https://docs.example/x"), &net).expect("fetch");
    assert!(
        !out.contains("truncated"),
        "a complete body carries no omission marker: {out}"
    );
    assert!(
        out.len() < webfetch::OUTPUT_MAX,
        "and it is nowhere near the output bound: {} bytes",
        out.len()
    );
    // **The count is the assertion**, because the bytes cannot tell the two
    // endings apart: one read for the body, one for the `None` that ends it.
    assert_eq!(
        net.reads(),
        2,
        "the loop ended on EOF, not on the input cap (which would cost {} reads)",
        webfetch::INPUT_MAX.div_ceil(64 * 1024)
    );
}

// Verifies: gh #351 finding 1 - the read-count row. A body delivered in N
// chunks reads exactly N+1 times, which is what proves the loop terminated on
// the transport's `None` rather than on an empty chunk or the cap.
//
// A chunk size that does not divide the body evenly is on purpose: the last
// data read is short, and the `None` after it must not be mistaken for it.
#[test]
fn a_chunked_body_costs_one_read_per_chunk_plus_the_eof() {
    let body = "x".repeat(1000);
    // (chunk size, the reads that must happen). Literal numbers rather than a
    // formula: a helper that recomputes the loop's arithmetic would agree with a
    // wrong loop.
    for (chunk, reads) in [(64, 17), (250, 5), (999, 3), (1000, 2)] {
        let net = FakeNet::in_chunks("https://docs.example/x", "text/plain", &body, chunk);
        let out = fetch(&request("https://docs.example/x"), &net).expect("fetch");
        assert_eq!(out, body, "the whole body arrived for chunk={chunk}");
        assert_eq!(
            net.reads(),
            reads,
            "one read per chunk, then one for the EOF (chunk={chunk})"
        );
    }
}

// Verifies: gh #351 finding 1 - a **zero-length body** is an immediate EOF: one
// read, an empty result, no marker. The edge the chunked fake makes reachable,
// and the one a real 204-ish response actually produces.
#[test]
fn a_zero_length_body_is_an_immediate_eof() {
    let net = FakeNet::route("https://docs.example/x", "text/html", "");
    let out = fetch(&request("https://docs.example/x"), &net).expect("fetch");
    assert_eq!(out, "");
    assert_eq!(net.reads(), 1, "one read answers `None`");
}

// Verifies: gh #351 finding 1 - both cuts keep their own scenario, and they are
// **distinguishable**. The output cut is about token length; the input cut is about
// content that was never read. They used to share one marker, and the input cut had
// no marker at all unless the text also exceeded 50 KiB.
#[test]
fn both_cuts_are_reported_and_with_their_own_marker() {
    // Output cut: a small body whose *text* is long. The whole page was read.
    let long_text = format!("<html><body><p>{}</p></body></html>", "x".repeat(100_000));
    let net = FakeNet::in_chunks("https://docs.example/x", "text/html", &long_text, 64 * 1024);
    let out = fetch(&request("https://docs.example/x"), &net).expect("fetch");
    assert!(out.contains(webfetch::TRUNCATION_MARKER), "{out:.200}");
    assert!(
        !out.contains(webfetch::INPUT_TRUNCATION_MARKER),
        "the page was read to the end, so only the output was cut"
    );
    assert!(
        out.len() <= webfetch::OUTPUT_MAX + 64,
        "and the output is bounded: {}",
        out.len()
    );

    // Input cut: the page continues past 1 MiB. Both markers are possible at once,
    // which is why the second is appended after `bound`.
    let big = format!("<html><body><p>{}</p></body></html>", "x".repeat(1_500_000));
    let net = FakeNet::in_chunks("https://docs.example/x", "text/html", &big, 64 * 1024);
    let out = fetch(&request("https://docs.example/x"), &net).expect("fetch");
    assert!(
        out.contains(webfetch::INPUT_TRUNCATION_MARKER),
        "the reader is told the page continued: {}",
        &out[out.len().saturating_sub(160)..]
    );
    assert!(
        out.len() <= webfetch::OUTPUT_MAX + webfetch::INPUT_TRUNCATION_MARKER.len() + 64,
        "and the output is bounded: {}",
        out.len()
    );
    // 16 reads fill the cap; the 17th is the one peek that proves the page did not
    // end there.
    assert_eq!(net.reads(), 17, "the cap, then one read past it");
}

// Verifies: gh #351 - **the defect this gap was hiding**, and the reason a test
// gap is not always just a test gap.
//
// With the input cut silent, a 1.4 MB page whose first 1 MiB was all `<script>` (which
// the converter strips) came back as an **empty string with no marker**: nothing to
// read, and no way to know why. Ordinary for a real page - a large inline bundle at the
// top of the document is exactly that shape - so `webfetch` was telling the model "this
// page is empty" about a page it had not finished reading.
#[test]
fn a_page_cut_before_its_content_says_so_instead_of_returning_nothing() {
    let page = format!(
        "<html><body><script>{}</script><p>THE ACTUAL ANSWER</p></body></html>",
        "// padding\n".repeat(200_000)
    );
    assert!(
        page.len() > webfetch::INPUT_MAX,
        "the fixture must cross the bound: {} bytes",
        page.len()
    );
    let net = FakeNet::in_chunks("https://docs.example/x", "text/html", &page, 64 * 1024);
    let out = fetch(&request("https://docs.example/x"), &net).expect("fetch");
    // The answer really was past the cut - that is what makes this the silent case.
    assert!(
        !out.contains("THE ACTUAL ANSWER"),
        "the content is beyond the bound: {out:?}"
    );
    assert!(
        out.contains(webfetch::INPUT_TRUNCATION_MARKER),
        "so the reader must be told, not handed an empty string: {out:?}"
    );
}

// Verifies: gh #351 - a page that ends **exactly** on the cap loses nothing, so it
// gets no input marker. Without this row the peek could be replaced by
// "`len >= INPUT_MAX` means truncated", which would mislabel a complete page as cut.
#[test]
fn a_page_that_ends_on_the_cap_is_not_reported_as_cut() {
    // Padding to land the body exactly on `INPUT_MAX`, then EOF.
    let wrapping = "<html><body><p></p></body></html>".len();
    let page = format!(
        "<html><body><p>{}</p></body></html>",
        "y".repeat(webfetch::INPUT_MAX - wrapping)
    );
    assert_eq!(
        page.len(),
        webfetch::INPUT_MAX,
        "the fixture ends on the cap"
    );
    let net = FakeNet::in_chunks("https://docs.example/x", "text/html", &page, 64 * 1024);
    let out = fetch(&request("https://docs.example/x"), &net).expect("fetch");
    assert!(
        !out.contains(webfetch::INPUT_TRUNCATION_MARKER),
        "the page ended where the cap did, so nothing was lost: {out:.120}"
    );
}

// Verifies: gh #351 finding 1 - a transport that answers `Some(empty)` for ever
// (out of contract: `None` means EOF) still ends the loop rather than spinning.
//
// The production loop breaks on any non-data answer, which is what makes the
// misbehaving adapter harmless. Pinned because the opposite - `Some(chunk) =>
// extend` with no empty check - is a hang of exactly this shape, and a hang in
// a tool call has no error to grep for.
#[test]
fn an_adapter_that_never_says_eof_still_ends() {
    let net = FakeNet::empty_forever("https://docs.example/x", "text/html");
    let out = fetch(&request("https://docs.example/x"), &net).expect("fetch");
    assert_eq!(out, "", "nothing was ever delivered");
    assert_eq!(net.reads(), 1, "the first empty answer ends the loop");
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
    struct Scripted {
        /// `false` until the one document this fake has is handed over: a
        /// scripted body is a stream like any other, so it ends at `None`
        /// rather than being re-delivered until the input cap (gh #351).
        delivered: RefCell<bool>,
    }
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
            // A new request is a new response body.
            *self.delivered.borrow_mut() = false;
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
            let mut delivered = self.delivered.borrow_mut();
            if *delivered {
                return Ok(None);
            }
            *delivered = true;
            Ok(Some(DOC.as_bytes().to_vec()))
        }
        fn close(&self, _h: u32) {}
    }
    CALLS.with(|calls| calls.borrow_mut().clear());
    let out = fetch(
        &request("https://docs.example/start"),
        &Scripted {
            delivered: RefCell::new(false),
        },
    )
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
        /// One document, handed over once per response (gh #351).
        delivered: RefCell<bool>,
    }
    impl NetView for Redirects {
        fn request(&self, _m: &str, _url: &str, _h: &[(&str, &str)]) -> Result<u32, String> {
            *self.hops.borrow_mut() += 1;
            *self.delivered.borrow_mut() = false;
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
            let mut delivered = self.delivered.borrow_mut();
            if *delivered {
                return Ok(None);
            }
            *delivered = true;
            Ok(Some(DOC.as_bytes().to_vec()))
        }
        fn close(&self, _h: u32) {}
    }
    let net = Redirects {
        hops: RefCell::new(0),
        delivered: RefCell::new(false),
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
