//! The HTTP stack the resolvers share (S3's ceiling split): one hyper
//! client, a plain GET, the OCI token dance, and the redirect-following
//! byte fetch.

use super::Error;

// ---------------------------------------------------------------------------
// HTTP: one client, the existing stack (the closed list's fallback for
// OCI: direct distribution calls over hyper)
// ---------------------------------------------------------------------------

type HttpsClient = hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    http_body_util::Full<hyper::body::Bytes>,
>;

pub(super) fn http_client() -> Result<HttpsClient, Error> {
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http() // http carries local/dev registries and the tests' loopback servers;
        // production refs are https (SRDD: fetched over HTTPS)
        .enable_http1()
        .build();
    Ok(
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(https),
    )
}

/// A plain GET for callers outside the OCI dance (the agent's daily
/// update check is the only one today): the same client stack, no
/// auth, the body as bytes. The User-Agent header is not optional at
/// the API that answers it - a request without one is refused.
pub async fn plain_get(url: &str) -> Result<Vec<u8>, Error> {
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
        .map_err(|err| Error::Fetch(err.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Fetch(format!("GET {url} answered {status}")));
    }
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .map_err(|err| Error::Fetch(err.to_string()))?
        .to_bytes();
    Ok(body.to_vec())
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
    async fn attempt(
        client: &HttpsClient,
        url: &str,
        accept: &str,
        headers: &hyper::HeaderMap,
    ) -> Result<hyper::Response<hyper::body::Incoming>, Error> {
        let mut request = hyper::Request::builder()
            .method("GET")
            .uri(url)
            .header(hyper::header::ACCEPT, accept);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let request = request
            .body(http_body_util::Full::new(hyper::body::Bytes::new()))
            .map_err(|err| Error::Fetch(err.to_string()))?;
        client
            .request(request)
            .await
            .map_err(|err| Error::Fetch(format!("{url}: {err}")))
    }

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

    // Follow redirects: ghcr answers blob GETs with a307 to its CDN,
    // where the signed URL needs no bearer (and must not get this
    // repository's token past its host).
    let mut response = attempt(client, url, accept, &authorization(token)?).await?;
    let mut hops = 0;
    while response.status().is_redirection() && hops < 5 {
        hops += 1;
        let location = response
            .headers()
            .get(hyper::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| Error::Fetch(format!("{url}: redirect with no location")))?
            .to_string();
        let next = if location.starts_with("http://") || location.starts_with("https://") {
            location
        } else {
            // Relative: stay on this origin.
            let origin_end = url.find("/v2/").unwrap_or(0);
            format!("{}{}", &url[..origin_end], location)
        };
        let same_origin = next
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            == url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .split('/')
                .next();
        let mut headers = authorization(token)?;
        if !same_origin {
            headers.remove(hyper::header::AUTHORIZATION);
        }
        response = attempt(client, &next, accept, &headers).await?;
        let _ = &next;
    }
    if response.status() != hyper::StatusCode::UNAUTHORIZED {
        if !response.status().is_success() {
            let status = response.status();
            return Err(Error::Fetch(format!("{url}: HTTP {status}")));
        }
        let headers = response.headers().clone();
        let body = collect_body(response).await?;
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
    let body = http_body_util::BodyExt::collect(token_response.into_body())
        .await
        .map_err(|err| Error::Fetch(format!("{token_url}: {err}")))?
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|err| Error::Fetch(format!("token endpoint is not JSON: {err}")))?;
    *token = json
        .get("token")
        .and_then(|t| t.as_str())
        .map(str::to_string);

    let retry = attempt(client, url, accept, &authorization(token)?).await?;
    if !retry.status().is_success() {
        let status = retry.status();
        return Err(Error::Fetch(format!(
            "{url}: HTTP {status} after authorization"
        )));
    }
    let headers = retry.headers().clone();
    let body = collect_body(retry).await?;
    Ok((body, headers))
}

/// Read a response body to bytes.
pub(super) async fn collect_body(
    mut response: hyper::Response<hyper::body::Incoming>,
) -> Result<Vec<u8>, Error> {
    let body = http_body_util::BodyExt::collect(response.body_mut())
        .await
        .map_err(|err| Error::Fetch(err.to_string()))?
        .to_bytes()
        .to_vec();
    Ok(body)
}

pub(super) async fn fetch_bytes(
    client: &HttpsClient,
    url: &str,
    accept: &str,
) -> Result<(Vec<u8>, hyper::HeaderMap), Error> {
    // Follow redirects: a release asset answers302 to its CDN, a blob
    // answers307 to the registry's store - both plain GETs with no
    // credential to drop.
    let mut url = url.to_string();
    let mut hops = 0;
    loop {
        let request = hyper::Request::builder()
            .method("GET")
            .uri(&url)
            .header(hyper::header::ACCEPT, accept)
            .body(http_body_util::Full::new(hyper::body::Bytes::new()))
            .map_err(|err| Error::Fetch(err.to_string()))?;
        let response = client
            .request(request)
            .await
            .map_err(|err| Error::Fetch(format!("{url}: {err}")))?;
        if response.status().is_redirection() && hops < 10 {
            hops += 1;
            url = response
                .headers()
                .get(hyper::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| Error::Fetch(format!("{url}: redirect with no location")))?
                .to_string();
            continue;
        }
        let status = response.status();
        let headers = response.headers().clone();
        if !status.is_success() {
            return Err(Error::Fetch(format!("{url}: HTTP {status}")));
        }
        let body = http_body_util::BodyExt::collect(response.into_body())
            .await
            .map_err(|err| Error::Fetch(format!("{url}: {err}")))?
            .to_bytes()
            .to_vec();
        return Ok((body, headers));
    }
}
