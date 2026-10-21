//! The HTTP stack the resolvers share (S3's ceiling split): one hyper
//! client, a plain GET, the OCI token dance, and the redirect-following
//! byte fetch.

use super::Error;

// ---------------------------------------------------------------------------
// HTTP: one client, the existing stack (the closed list's fallback for
// OCI: direct distribution calls over hyper)
// ---------------------------------------------------------------------------

type HttpsClient = hyper_util::client::legacy::Client<
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

// ---------------------------------------------------------------------------
// Proxy routing (gh #308): the `net` client's rules, in this crate
// ---------------------------------------------------------------------------
//
// `lca-tools`' `proxy` module is private and its `ProxyTcp` is typed over that
// crate's pinned DNS resolver, so it is not reachable from here without
// widening another crate's API (or, worse, forking a second parser that
// drifts). The rule set below is deliberately the same rule set: env read once
// per client build, uppercase-then-lowercase, `ALL_PROXY` as the fallback for
// both schemes, curl's `NO_PROXY` tail match, and a `CONNECT` tunnel for
// https with proxy credentials from the URL userinfo. Credentials are never
// logged here or anywhere else.

/// One proxy to dial: host, port, and optional `user:pass` credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyEndpoint {
    /// The proxy's hostname or IP literal.
    pub host: String,
    /// The proxy's port (curl's 1080 when the variable names none).
    pub port: u16,
    /// Raw `user:pass` credentials from the URL userinfo, when present.
    pub auth: Option<String>,
}

/// The process proxy configuration: one endpoint per scheme plus the bypass
/// list.
#[derive(Debug, Clone, Default)]
pub struct Proxies {
    http: Option<ProxyEndpoint>,
    https: Option<ProxyEndpoint>,
    no_proxy: Vec<String>,
}

impl Proxies {
    /// Read the proxy configuration from the process environment.
    pub fn from_env() -> Proxies {
        let get =
            |upper: &str, lower: &str| std::env::var(upper).or_else(|_| std::env::var(lower)).ok();
        let all = get("ALL_PROXY", "all_proxy");
        let parse = |value: Option<String>| value.as_deref().and_then(parse_proxy);
        Proxies {
            http: parse(get("HTTP_PROXY", "http_proxy").or_else(|| all.clone())),
            https: parse(get("HTTPS_PROXY", "https_proxy").or(all)),
            no_proxy: get("NO_PROXY", "no_proxy")
                .map(split_list)
                .unwrap_or_default(),
        }
    }

    /// Build a configuration directly (the tests' seam: no process environment
    /// to mutate, which would race every other row in the binary).
    pub fn new(
        http: Option<ProxyEndpoint>,
        https: Option<ProxyEndpoint>,
        no_proxy: Vec<String>,
    ) -> Proxies {
        Proxies {
            http,
            https,
            no_proxy,
        }
    }

    /// The endpoint for one request, if any: the per-scheme variable (or the
    /// `ALL_PROXY` fallback) minus the `NO_PROXY` bypass list.
    pub fn endpoint_for(&self, scheme: &str, host: &str, port: u16) -> Option<&ProxyEndpoint> {
        let endpoint = match scheme {
            "http" => self.http.as_ref(),
            "https" => self.https.as_ref(),
            _ => None,
        }?;
        if self.bypasses(host, port) {
            return None;
        }
        Some(endpoint)
    }

    /// Whether `NO_PROXY` bypasses this target (curl's tail match: an entry
    /// matches its exact host and every subdomain; a `:port` qualifier
    /// restricts to that port; `*` bypasses everything).
    pub fn bypasses(&self, host: &str, port: u16) -> bool {
        let host = host.to_ascii_lowercase();
        for entry in &self.no_proxy {
            let entry = entry.trim().to_ascii_lowercase();
            if entry == "*" {
                return true;
            }
            let (mut name, entry_port) = match entry.rsplit_once(':') {
                Some((name, port)) if !name.is_empty() && port.parse::<u16>().is_ok() => {
                    (name, Some(port))
                }
                _ => (entry.as_str(), None),
            };
            if let Some(entry_port) = entry_port
                && entry_port.parse::<u16>().ok() != Some(port)
            {
                continue;
            }
            name = name.trim_start_matches('.');
            if name.is_empty() {
                continue;
            }
            if host == name || host.ends_with(&format!(".{name}")) {
                return true;
            }
        }
        false
    }
}

/// Parse one proxy variable (the tests' seam: [`parse_proxy`] is private, and a
/// row that pins the env parser's behaviour is cheaper than one that mutates
/// the process environment).
pub fn parse_proxy_for_tests(value: &str) -> Option<ProxyEndpoint> {
    parse_proxy(value)
}

fn split_list(value: String) -> Vec<String> {
    value
        .split(',')
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// Parse one proxy variable into an endpoint: a bare `host:port` gains the
/// `http://` scheme curl assumes; anything unparseable (or a non-http scheme,
/// proxy-to-proxy TLS being out of scope) is ignored rather than fatal, exactly
/// like an unset variable.
fn parse_proxy(value: &str) -> Option<ProxyEndpoint> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let with_scheme = if value.contains("://") {
        value.to_string()
    } else {
        format!("http://{value}")
    };
    let uri: hyper::Uri = with_scheme.parse().ok()?;
    if uri.scheme_str() != Some("http") {
        return None;
    }
    let authority = uri.authority()?;
    let raw = authority.as_str();
    let (userinfo, hostport) = match raw.rsplit_once('@') {
        Some((user, rest)) => (Some(user.to_string()), rest),
        None => (None, raw),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => (host.to_string(), port),
            Err(_) => (hostport.to_string(), 1080),
        },
        None => (hostport.to_string(), 1080),
    };
    if host.is_empty() {
        return None;
    }
    Some(ProxyEndpoint {
        host,
        port,
        auth: userinfo.filter(|user| !user.is_empty()),
    })
}

/// Standard base-64 for `Proxy-Authorization` (hand-rolled, as the `net`
/// client does: the only use is one header value).
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let mut word = 0u32;
        for byte in chunk {
            word = (word << 8) | u32::from(*byte);
        }
        word <<= 8 * (3 - chunk.len());
        let digits = chunk.len() + 1;
        for index in 0..4 {
            if index < digits {
                out.push(ALPHABET[((word >> (18 - 6 * index)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The TCP layer under rustls: dials the proxy when the environment selects
/// one, tunnels `CONNECT` for https, and delegates untouched otherwise. Always
/// in the stack (one connector type), so proxied and direct paths share every
/// line above it.
#[derive(Clone)]
pub struct ProxyTcp {
    inner: hyper_util::client::legacy::connect::HttpConnector,
    proxies: Proxies,
}

impl ProxyTcp {
    /// Wrap the plain connector with a proxy configuration.
    pub fn new(
        inner: hyper_util::client::legacy::connect::HttpConnector,
        proxies: Proxies,
    ) -> ProxyTcp {
        ProxyTcp { inner, proxies }
    }

    /// The proxy dial URI for an endpoint (`http://host:port`, pathless: the
    /// connector connects to the authority and ignores the rest).
    fn dial_uri(endpoint: &ProxyEndpoint) -> std::io::Result<hyper::Uri> {
        format!("http://{}:{}", endpoint.host, endpoint.port)
            .parse()
            .map_err(|err| std::io::Error::other(format!("bad proxy address: {err}")))
    }
}

impl tower_service::Service<hyper::Uri> for ProxyTcp {
    type Response = ProxyStream;
    type Error = std::io::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner
            .poll_ready(cx)
            .map_err(|err| std::io::Error::other(err.to_string()))
    }

    fn call(&mut self, uri: hyper::Uri) -> Self::Future {
        let scheme = uri.scheme_str().unwrap_or("").to_string();
        let host = uri.host().unwrap_or("").to_ascii_lowercase();
        let port = uri
            .port_u16()
            .unwrap_or(if scheme == "https" { 443 } else { 80 });
        let endpoint = self.proxies.endpoint_for(&scheme, &host, port).cloned();
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let io_error = |err: &dyn std::fmt::Display| std::io::Error::other(err.to_string());
            let Some(endpoint) = endpoint else {
                return inner
                    .call(uri)
                    .await
                    .map(ProxyStream::direct)
                    .map_err(|err| io_error(&err));
            };
            let proxy_uri = Self::dial_uri(&endpoint)?;
            if scheme == "https" {
                // hyper-util's CONNECT tunnel; the rustls layer above
                // handshakes inside it, so the connection reports unproxied
                // (origin form inside TLS).
                let mut tunnel =
                    hyper_util::client::legacy::connect::proxy::Tunnel::new(proxy_uri, inner);
                if let Some(credentials) = endpoint.auth.as_deref() {
                    let value = hyper::header::HeaderValue::from_str(&format!(
                        "Basic {}",
                        base64_encode(credentials.as_bytes())
                    ))
                    .map_err(|err| io_error(&err))?;
                    tunnel = tunnel.with_auth(value);
                }
                return tunnel
                    .call(uri)
                    .await
                    .map(ProxyStream::tunneled)
                    .map_err(|err| io_error(&err));
            }
            // Plain http: the connection IS to the proxy, so the client writes
            // the absolute form (and `Host` still names the target).
            inner
                .call(proxy_uri)
                .await
                .map(ProxyStream::proxied)
                .map_err(|err| io_error(&err))
        })
    }
}

/// One established TCP stream plus whether it ends at a proxy: the flag becomes
/// hyper-util's `Connected::proxy`, the only thing that flips a request between
/// origin and absolute form. HTTPS tunnels report unproxied - inside TLS the
/// client talks to the origin.
pub struct ProxyStream {
    inner: hyper_util::rt::TokioIo<tokio::net::TcpStream>,
    proxied: bool,
}

impl ProxyStream {
    fn direct(inner: hyper_util::rt::TokioIo<tokio::net::TcpStream>) -> ProxyStream {
        ProxyStream {
            inner,
            proxied: false,
        }
    }

    fn proxied(inner: hyper_util::rt::TokioIo<tokio::net::TcpStream>) -> ProxyStream {
        ProxyStream {
            inner,
            proxied: true,
        }
    }

    fn tunneled(inner: hyper_util::rt::TokioIo<tokio::net::TcpStream>) -> ProxyStream {
        ProxyStream {
            inner,
            proxied: false,
        }
    }
}

impl hyper::rt::Read for ProxyStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl hyper::rt::Write for ProxyStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

impl hyper_util::client::legacy::connect::Connection for ProxyStream {
    fn connected(&self) -> hyper_util::client::legacy::connect::Connected {
        hyper_util::client::legacy::connect::Connected::new().proxy(self.proxied)
    }
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

/// The most one OCI document may retain, and how long one fetch may take
/// (gh #308). A manifest or config is kilobytes; a layer archive is bounded
/// separately by [`crate::MAX_ARCHIVE_BYTES`]. The cap is enforced *while
/// reading*, because the digest is verified only after the bytes land - an
/// unbounded collect is a pre-verification memory hazard.
pub const OCI_MAX_BYTES: usize = 32 * 1024 * 1024;

/// How long one OCI fetch may take, end to end (gh #308): the same
/// installer-grade posture [`PLAIN_GET_TIMEOUT`] uses (gate 10).
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
/// The public seam for tests and any future caller: [`fetch_oci`] is this
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
