//! The OCI distribution protocol (FR-DIST-1): the bearer-token dance, the
//! redirect-following fetch, the bounded body collector, and the archive fetch.
//!
//! Anonymous pulls only (the risk table's decision). Every bound here is shipped,
//! not test-only: the public seam takes the constants production passes, so a row
//! that exercises a cap exercises what ships (gh #308/#355).

use super::Error;
use super::http::{HttpsClient, http_client};

/// The most one OCI document may retain, and how long one fetch may take
/// (gh #308). A manifest or config is kilobytes; a layer archive is bounded
/// separately by [`crate::MAX_ARCHIVE_BYTES`]. The cap is enforced *while
/// reading*, because the digest is verified only after the bytes land - an
/// unbounded collect is a pre-verification memory hazard.
pub const OCI_MAX_BYTES: usize = 32 * 1024 * 1024;

/// How long one OCI fetch may take, end to end (gh #308): the same
/// installer-grade posture [`crate::PLAIN_GET_TIMEOUT`] uses (gate 10).
pub const OCI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long one archive download may take (gh #308). An extension archive is
/// up to [`crate::MAX_ARCHIVE_BYTES`], so it gets a longer deadline than a
/// JSON document but still a finite one.
pub const ARCHIVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// How many redirects one fetch chain may follow (gh #308). The pre-fix code
/// used 5 on the initial attempt and none on the retry; one budget now covers
/// both, and 5 is what a registry -> CDN -> object-store chain needs.
pub const OCI_MAX_REDIRECTS: u32 = 5;

/// Resolve a reference through the OCI dance with the shipped bounds.
///
/// The public seam for tests and any future caller: the inner `fetch_oci` is this
/// function taking the client and the mutable token cache, so an exercised
/// bound is the shipped bound (the `plain_get` shape, gh #355).
pub async fn fetch_oci_public(
    url: &str,
    accept: &str,
    image: &str,
) -> Result<(Vec<u8>, hyper::HeaderMap), Error> {
    fetch_oci_within(url, accept, image, OCI_TIMEOUT, OCI_MAX_BYTES).await
}

/// [`fetch_oci_public`] with an explicit deadline and cap (the tests' seam).
pub async fn fetch_oci_within(
    url: &str,
    accept: &str,
    image: &str,
    timeout: std::time::Duration,
    max_bytes: usize,
) -> Result<(Vec<u8>, hyper::HeaderMap), Error> {
    fetch_oci_within_capped(url, accept, image, timeout, max_bytes).await
}

/// [`fetch_oci_public`] with an explicit deadline *and* cap (the tests' seam).
///
/// One code path: the production entry point passes the shipped constants, so
/// a row that exercises the timeout or the cap exercises what ships.
pub async fn fetch_oci_within_capped(
    url: &str,
    accept: &str,
    image: &str,
    timeout: std::time::Duration,
    max_bytes: usize,
) -> Result<(Vec<u8>, hyper::HeaderMap), Error> {
    let client = http_client()?;
    let mut token: Option<String> = None;
    let work =
        async { fetch_oci_once(&client, url, accept, image, &mut token, timeout, max_bytes).await };
    match tokio::time::timeout(timeout, work).await {
        Ok(outcome) => outcome,
        Err(_) => Err(Error::Fetch(format!(
            "{url}: the registry did not answer within {timeout:?} - check the network, or the registry may be down"
        ))),
    }
}

/// One GET with the given headers, following redirects up to
/// [`OCI_MAX_REDIRECTS`].
///
/// **This is the one redirect loop** (gh #308 defect 1): the initial attempt
/// and the post-401 retry both go through it, so they cannot disagree about
/// the budget or about what a redirect does to the bearer. A redirect to a
/// *different host* is the normal ghcr shape (a blob URL on the CDN), and the
/// bearer is dropped there: the signed URL does not need it and forwarding a
/// repository token past its host is the leak gh #349 was about.
async fn get_following_redirects(
    client: &HttpsClient,
    url: &str,
    accept: &str,
    headers: &hyper::HeaderMap,
) -> Result<hyper::Response<hyper::body::Incoming>, Error> {
    let origin_of = |target: &str| -> String {
        target
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let mut target = url.to_string();
    let mut current = headers.clone();
    let mut hops = 0u32;
    loop {
        let mut request = hyper::Request::builder()
            .method("GET")
            .uri(&target)
            .header(hyper::header::ACCEPT, accept);
        for (name, value) in &current {
            request = request.header(name, value);
        }
        let request = request
            .body(http_body_util::Full::new(hyper::body::Bytes::new()))
            .map_err(|err| Error::Fetch(err.to_string()))?;
        let response = client
            .request(request)
            .await
            .map_err(|err| Error::Fetch(format!("{target}: {err}")))?;
        if !response.status().is_redirection() {
            return Ok(response);
        }
        if hops >= OCI_MAX_REDIRECTS {
            return Err(Error::Fetch(format!(
                "{url}: followed {OCI_MAX_REDIRECTS} redirects without reaching the artifact - the registry is redirecting in a loop"
            )));
        }
        hops += 1;
        let location = response
            .headers()
            .get(hyper::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| Error::Fetch(format!("{target}: redirect with no location")))?
            .to_string();
        let next = if location.starts_with("http://") || location.starts_with("https://") {
            location
        } else {
            // Relative: stay on this origin.
            let origin_end = target.find("/v2/").unwrap_or(0);
            format!("{}{}", &target[..origin_end], location)
        };
        if origin_of(&next) != origin_of(&target) {
            current.remove(hyper::header::AUTHORIZATION);
        }
        target = next;
    }
}

/// The OCI token dance: registries answer an unauthenticated request
/// with `WWW-Authenticate: Bearer realm=...,service=...`, the client
/// fetches a (possibly anonymous) bearer token from that realm, and
/// retries once. ghcr.io requires this even for public pulls; the local
/// mock registries in the tests answer without it, which is why this
/// rides along instead of a token being mandatory.
pub(super) async fn fetch_oci(
    client: &HttpsClient,
    url: &str,
    accept: &str,
    image: &str,
    token: &mut Option<String>,
) -> Result<(Vec<u8>, hyper::HeaderMap), Error> {
    fetch_oci_once(
        client,
        url,
        accept,
        image,
        token,
        OCI_TIMEOUT,
        OCI_MAX_BYTES,
    )
    .await
}

/// [`fetch_oci`] with explicit bounds; the caller owns the deadline.
async fn fetch_oci_once(
    client: &HttpsClient,
    url: &str,
    accept: &str,
    image: &str,
    token: &mut Option<String>,
    timeout: std::time::Duration,
    max_bytes: usize,
) -> Result<(Vec<u8>, hyper::HeaderMap), Error> {
    let authorization = |token: &Option<String>| -> Result<hyper::HeaderMap, Error> {
        let mut headers = hyper::HeaderMap::new();
        if let Some(value) = token {
            let header =
                hyper::header::HeaderValue::from_str(&format!("Bearer {value}")).map_err(|_| {
                    Error::Fetch("registry returned an invalid authorization token".to_string())
                })?;
            headers.insert(hyper::header::AUTHORIZATION, header);
        }
        Ok(headers)
    };

    // The initial attempt: same loop, same budget as the retry below.
    let response = get_following_redirects(client, url, accept, &authorization(token)?).await?;
    if response.status() != hyper::StatusCode::UNAUTHORIZED {
        if !response.status().is_success() {
            let status = response.status();
            return Err(Error::Fetch(format!("{url}: HTTP {status}")));
        }
        let headers = response.headers().clone();
        let body = collect_body(response, max_bytes).await?;
        return Ok((body, headers));
    }

    // Challenge -> token -> one retry, the spec's dance.
    let challenge = response
        .headers()
        .get(hyper::header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    // The header starts with its auth scheme (`Bearer realm=...`); the
    // parameters begin after it.
    let parameters = challenge
        .strip_prefix("Bearer ")
        .unwrap_or(challenge.as_str());
    let field = |name: &str| {
        parameters.split(',').find_map(|part| {
            let part = part.trim();
            part.strip_prefix(&format!("{name}="))
                .map(|value| value.trim().trim_matches('"').to_string())
        })
    };
    let realm = field("realm")
        .ok_or_else(|| Error::Fetch(format!("{url}: HTTP401 with no usable challenge")))?;
    let service = field("service").unwrap_or_default();
    let scope = format!("repository:{image}:pull");
    let token_url = format!(
        "{realm}{}service={service}&scope={scope}",
        if realm.contains('?') { "&" } else { "?" }
    );
    let mut token_request = hyper::Request::builder().method("GET").uri(&token_url);
    if let Some(value) = &token {
        let header =
            hyper::header::HeaderValue::from_str(&format!("Bearer {value}")).map_err(|_| {
                Error::Fetch("registry returned an invalid authorization token".to_string())
            })?;
        token_request = token_request.header(hyper::header::AUTHORIZATION, header);
    }
    let token_request = token_request
        .body(http_body_util::Full::new(hyper::body::Bytes::new()))
        .map_err(|err| Error::Fetch(err.to_string()))?;
    let token_response = client
        .request(token_request)
        .await
        .map_err(|err| Error::Fetch(format!("{token_url}: {err}")))?;
    if !token_response.status().is_success() {
        return Err(Error::Fetch(format!(
            "{url}: HTTP401 and the token endpoint answered {}",
            token_response.status()
        )));
    }
    let body = collect_body_bounded(token_response, max_bytes, &token_url, OCI_TIMEOUT).await?;
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|err| Error::Fetch(format!("token endpoint is not JSON: {err}")))?;
    // gh #308 defect 2: the distribution spec permits `access_token` where
    // ghcr answers `token`. Reading only `token` left the retry
    // unauthenticated on any registry that answers the spec's other field.
    let issued = json
        .get("token")
        .and_then(|value| value.as_str())
        .or_else(|| json.get("access_token").and_then(|value| value.as_str()));
    let Some(issued) = issued else {
        // Not a silent unauthenticated retry: quote what the endpoint
        // actually answered, which is the only way to tell "you need a
        // different scope" from "this challenge is malformed".
        let answer: String = String::from_utf8_lossy(&body).chars().take(200).collect();
        return Err(Error::Fetch(format!(
            "{url}: HTTP401 and the token endpoint answered no `token` and no `access_token` field - it said: {answer}"
        )));
    };
    *token = Some(issued.to_string());

    // The retry: the SAME loop, so a 307 to the CDN (the normal ghcr blob
    // shape) is followed rather than reported as `HTTP 307 after
    // authorization` (gh #308 defect 1).
    let retry = get_following_redirects(client, url, accept, &authorization(token)?).await?;
    if !retry.status().is_success() {
        let status = retry.status();
        return Err(Error::Fetch(format!(
            "{url}: HTTP {status} after authorization"
        )));
    }
    let headers = retry.headers().clone();
    let body = collect_body(retry, max_bytes).await?;
    let _ = timeout;
    Ok((body, headers))
}

/// Read a response body to bytes, refusing one over `max_bytes` **while
/// reading** (gh #308 defect 3): the digest is verified only after the bytes
/// land, so an unbounded collect is a pre-verification memory hazard.
pub(super) async fn collect_body(
    response: hyper::Response<hyper::body::Incoming>,
    max_bytes: usize,
) -> Result<Vec<u8>, Error> {
    collect_body_bounded(response, max_bytes, "", OCI_TIMEOUT).await
}

/// [`collect_body`] naming the request in its refusals.
async fn collect_body_bounded(
    response: hyper::Response<hyper::body::Incoming>,
    max_bytes: usize,
    url: &str,
    timeout: std::time::Duration,
) -> Result<Vec<u8>, Error> {
    use http_body_util::BodyExt as _;
    let target = if url.is_empty() { "the registry" } else { url };
    // A declared length over the cap is refused before reading a byte.
    if let Some(length) = response
        .headers()
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.parse::<u64>().ok())
        && length > max_bytes as u64
    {
        return Err(Error::Fetch(format!(
            "{target}: the response declares {length} bytes, over the {max_bytes} byte limit for this fetch - the registry is serving something other than the artifact"
        )));
    }
    let mut body = response.into_body();
    let mut collected: Vec<u8> = Vec::new();
    let read = async {
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|err| Error::Fetch(format!("{target}: {err}")))?;
            let Some(chunk) = frame.data_ref() else {
                continue; // trailers and the like carry no payload
            };
            if collected.len() + chunk.len() > max_bytes {
                return Err(Error::Fetch(format!(
                    "{target}: the response passed the {max_bytes} byte limit for this fetch - the registry is serving something other than the artifact"
                )));
            }
            collected.extend_from_slice(chunk);
        }
        Ok(collected)
    };
    match tokio::time::timeout(timeout, read).await {
        Ok(outcome) => outcome,
        Err(_) => Err(Error::Fetch(format!(
            "{target}: the body did not arrive within {timeout:?} - the registry stalled mid-transfer"
        ))),
    }
}

pub(super) async fn fetch_bytes(
    client: &HttpsClient,
    url: &str,
    accept: &str,
) -> Result<(Vec<u8>, hyper::HeaderMap), Error> {
    // A release asset answers 302 to its CDN, a blob answers 307 to the
    // registry's store - both plain GETs with no credential to drop. Same
    // loop and budget as the OCI path since gh #308: two redirect policies in
    // one file is how the post-401 retry ended up with none.
    let empty = hyper::HeaderMap::new();
    let work = async {
        let response = get_following_redirects(client, url, accept, &empty).await?;
        let status = response.status();
        let headers = response.headers().clone();
        if !status.is_success() {
            return Err(Error::Fetch(format!("{url}: HTTP {status}")));
        }
        // Gh #350: bound what one download may retain, and do it while reading
        // (gh #308). The member ceilings in `read_archive` bound the *expanded*
        // size; this bounds the bytes that arrive before any of that runs.
        let body = collect_body_bounded(
            response,
            crate::MAX_ARCHIVE_BYTES as usize,
            url,
            ARCHIVE_TIMEOUT,
        )
        .await?;
        Ok((body, headers))
    };
    match tokio::time::timeout(ARCHIVE_TIMEOUT, work).await {
        Ok(outcome) => outcome,
        Err(_) => Err(Error::Fetch(format!(
            "{url}: the archive did not arrive within {ARCHIVE_TIMEOUT:?} - the host stalled mid-download"
        ))),
    }
}
