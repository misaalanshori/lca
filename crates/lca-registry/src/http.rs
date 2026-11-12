//! The HTTP stack the resolvers share (S3's ceiling split): one hyper client and
//! the plain GET, with its shipped timeout and cap (gh #355).
//!
//! The proxy rules are in `proxy.rs` and the OCI protocol in `oci.rs` (gh #561):
//! this file is the client every fetch in the crate is built on, and the one plain
//! GET the update check, the release feed, the installer and the catalog all use.

use super::Error;
use super::proxy::{Proxies, ProxyTcp};

// ---------------------------------------------------------------------------
// HTTP: one client, the existing stack (the closed list's fallback for
// OCI: direct distribution calls over hyper)
// ---------------------------------------------------------------------------

pub(super) type HttpsClient = hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<ProxyTcp>,
    http_body_util::Full<hyper::body::Bytes>,
>;

pub(super) fn http_client() -> Result<HttpsClient, Error> {
    let mut http = hyper_util::client::legacy::connect::HttpConnector::new();
    // The wrapped connector must accept `https`: hyper-rustls's `build()`
    // clears this on a connector it creates, but `wrap_connector` leaves a
    // caller-supplied one as-is, and the default `enforce_http` rejects the
    // scheme before TLS is considered.
    http.enforce_http(false);
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http() // http carries local/dev registries and the tests' loopback servers;
        // production refs are https (SRDD: fetched over HTTPS)
        .enable_http1()
        // gh #308: the proxy layer sits under rustls, over the plain
        // connector - the same placement and the same env rules the `net`
        // capability client uses (gh #145), so one environment variable
        // routes both.
        .wrap_connector(ProxyTcp::new(http, Proxies::from_env()));
    Ok(
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(https),
    )
}

/// How long one `plain_get` may take, end to end (gh #355).
///
/// The self-update paths used to inherit "wait forever": a stalled release
/// feed hung `lca update --check` and `--changelog` indefinitely. 30 seconds
/// is the installer's own network posture (gate 10's script, which these
/// downloads feed), and it is generous for a JSON feed or an installer script.
pub const PLAIN_GET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The most one `plain_get` body may retain (gh #355).
///
/// A release feed and an installer script are kilobytes; 8 MiB is three
/// orders of magnitude of headroom. The cap matters most on
/// `install_binary`, which used to collect a hostile mirror's whole body into
/// memory *before* the installer's verify-then-move path could run.
pub const PLAIN_GET_MAX_BYTES: usize = 8 * 1024 * 1024;

/// A plain GET for callers outside the OCI dance: the agent's update check,
/// the release feed, the installer, and the catalog documents. The same
/// client stack, no auth, the body as bytes. The User-Agent header is not
/// optional at the API that answers it - a request without one is refused.
///
/// **Bounded in time and size** (gh #355): [`PLAIN_GET_TIMEOUT`] covers the
/// whole request *and* body read, and [`PLAIN_GET_MAX_BYTES`] is enforced
/// while the frames arrive, so an endless or oversized body is refused
/// instead of accumulated. Callers inherit both by construction; the one
/// caller with a tighter need (the daily check, FR-CFG-6's 5 s) wraps this
/// from outside, which still works because the tighter bound wins.
pub async fn plain_get(url: &str) -> Result<Vec<u8>, Error> {
    plain_get_within_capped(url, PLAIN_GET_TIMEOUT, PLAIN_GET_MAX_BYTES).await
}

/// [`plain_get`] with an explicit deadline (the tests' seam).
///
/// One code path: `plain_get` is this function with the production constants,
/// so a test that exercises the timeout also exercises what ships.
pub async fn plain_get_within(url: &str, timeout: std::time::Duration) -> Result<Vec<u8>, Error> {
    plain_get_within_capped(url, timeout, PLAIN_GET_MAX_BYTES).await
}

/// [`plain_get`] with an explicit deadline *and* cap (the tests' seam).
pub async fn plain_get_within_capped(
    url: &str,
    timeout: std::time::Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, Error> {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .to_string();
    let work = async {
        let client = http_client()?;
        let request = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(url)
            .header(
                hyper::header::USER_AGENT,
                concat!("lca/", env!("CARGO_PKG_VERSION")),
            )
            .header(hyper::header::ACCEPT, "application/json")
            .body(http_body_util::Full::new(hyper::body::Bytes::new()))
            .map_err(|err| Error::Fetch(err.to_string()))?;
        let response = client
            .request(request)
            .await
            .map_err(|err| Error::Fetch(format!("GET {host}: {err}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Fetch(format!("GET {url} answered {status}")));
        }
        // A declared length over the cap is refused before reading a byte.
        if let Some(length) = response
            .headers()
            .get(hyper::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.parse::<u64>().ok())
            && length > max_bytes as u64
        {
            return Err(too_large(url, length, max_bytes));
        }
        // Streamed, so the cap is a *stop* rather than a post-mortem: an
        // endless body is cut off at the limit instead of being collected in
        // full and then judged (which is what let a hostile mirror exhaust
        // memory, gh #355).
        use http_body_util::BodyExt as _;
        let mut body = response.into_body();
        let mut collected: Vec<u8> = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|err| Error::Fetch(format!("GET {host}: {err}")))?;
            let Some(chunk) = frame.data_ref() else {
                continue; // trailers and the like carry no payload
            };
            if collected.len() + chunk.len() > max_bytes {
                return Err(too_large(
                    url,
                    (collected.len() + chunk.len()) as u64,
                    max_bytes,
                ));
            }
            collected.extend_from_slice(chunk);
        }
        Ok(collected)
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(outcome) => outcome,
        Err(_) => Err(Error::Fetch(format!(
            "GET {host}: the host did not answer within {}s - check the network, or the host may be down",
            timeout.as_secs()
        ))),
    }
}

/// The refusal for a body over the cap: names the host and the limit, and says
/// what to do (the house refusal-copy style).
fn too_large(url: &str, bytes: u64, max_bytes: usize) -> Error {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url);
    Error::Fetch(format!(
        "GET {host}: the response is {bytes} bytes, over the {max_bytes} byte limit for this fetch -          the host is serving something other than the file expected; check the feed URL or the mirror"
    ))
}
