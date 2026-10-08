//! Env-driven proxy routing for the `net` host client (gh #145, pi's
//! `HTTP_PROXY`/`HTTPS_PROXY` row via undici's `EnvHttpProxyAgent`).
//!
//! The connector below wraps the pinned `HttpConnector` at the TCP
//! level, under the rustls layer, out of pieces already in the graph:
//! plain-http targets dial the proxy and report the connection as
//! proxied (hyper-util then writes the absolute form a forwarding
//! proxy routes on), while https targets go through hyper-util's
//! `Tunnel` (`CONNECT`, then the TLS handshake runs inside — pi's
//! `proxyTunnel` shape). With no proxy variables set the wrapper
//! delegates untouched, so normal paths keep their exact behavior
//! (and the rebinding preflight in `net_request` still validates the
//! target through direct DNS even when proxied: a network without
//! direct DNS stays refused, because dropping the check for proxy
//! users would weaken the sandbox).
//!
//! Environment is read once per client build (`Capabilities::new`),
//! so a proxy change needs a restart; pi reads per request, which is
//! the documented difference. Uppercase wins, lowercase is honored
//! (curl convention), `ALL_PROXY` is the fallback for both schemes.

use super::*;

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

/// The process proxy configuration: one endpoint per scheme plus the
/// bypass list.
#[derive(Debug, Clone, Default)]
pub struct Proxies {
    http: Option<ProxyEndpoint>,
    https: Option<ProxyEndpoint>,
    no_proxy: Vec<String>,
}

/// Read the proxy configuration from the process environment.
pub fn proxies_from_env() -> Proxies {
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

fn split_list(value: String) -> Vec<String> {
    value
        .split(',')
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// Parse one proxy variable into an endpoint: a bare `host:port` gains
/// the `http://` scheme curl assumes; anything unparseable (or a
/// non-http scheme — proxy-to-proxy TLS is out of scope) is ignored
/// rather than fatal, exactly like an unset variable.
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
    // ponytail: `Authority::host` drops brackets but not the port; the
    // last colon separates it (an IPv6 literal keeps its brackets).
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

impl Proxies {
    /// The endpoint for one request, if any: the per-scheme variable
    /// (or the `ALL_PROXY` fallback) minus the `NO_PROXY` bypass list.
    pub fn endpoint_for(&self, scheme: &str, host: &str, port: u16) -> Option<&ProxyEndpoint> {
        let endpoint = match scheme {
            "http" => self.http.as_ref(),
            "https" => self.https.as_ref(),
            _ => None,
        }?;
        if bypasses(&self.no_proxy, host, port) {
            return None;
        }
        Some(endpoint)
    }
}

/// Whether `NO_PROXY` bypasses this target (curl's tail match: an
/// entry matches its exact host and every subdomain; a `:port`
/// qualifier restricts to that port; `*` bypasses everything).
fn bypasses(no_proxy: &[String], host: &str, port: u16) -> bool {
    let host = host.to_ascii_lowercase();
    for entry in no_proxy {
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

/// Standard base-64 for `Proxy-Authorization` (hand-rolled: the only
/// use is one header value, and a whole crate for it loses the
/// ladder to the standard library's absence, not to size).
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

/// The TCP layer under rustls: dials the proxy when the environment
/// selects one, tunnels `CONNECT` for https, and delegates untouched
/// otherwise. Always in the stack (one connector type), so proxied
/// and direct paths share every line above this one.
#[derive(Clone)]
pub struct ProxyTcp {
    inner: HttpConnector<PinnedResolver>,
    proxies: Proxies,
}

impl ProxyTcp {
    /// Wrap the pinned connector with the current process proxy
    /// configuration.
    pub fn new(inner: HttpConnector<PinnedResolver>, proxies: Proxies) -> ProxyTcp {
        ProxyTcp { inner, proxies }
    }

    /// The proxy dial URI for an endpoint (`http://host:port`, pathless:
    /// [`HttpConnector`] connects to the authority and ignores the rest).
    /// A `Result`, not a panic: the host came from the environment, and a
    /// value that parsed once can still fail rebuilt (an unbracketed IPv6
    /// literal), so the caller reports it like any other dial failure.
    fn dial_uri(endpoint: &ProxyEndpoint) -> std::io::Result<hyper::Uri> {
        format!("http://{}:{}", endpoint.host, endpoint.port)
            .parse()
            .map_err(|err| std::io::Error::other(format!("bad proxy address: {err}")))
    }
}

impl tower_service::Service<hyper::Uri> for ProxyTcp {
    type Response = ProxyStream;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

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
            // The proxy TCP itself resolves through the pinned
            // resolver (no pin exists for a proxy host, so this is
            // ordinary system DNS, exactly like the direct path's).
            let proxy_uri = Self::dial_uri(&endpoint)?;
            if scheme == "https" {
                // hyper-util's CONNECT tunnel; the rustls layer above
                // handshakes inside it, so the connection reports
                // unproxied (origin form inside TLS).
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
            // Plain http: the connection IS to the proxy, so the
            // client writes the absolute form (and `Host` still names
            // the target, set from the request URI at dispatch).
            inner
                .call(proxy_uri)
                .await
                .map(ProxyStream::proxied)
                .map_err(|err| io_error(&err))
        })
    }
}

/// One established TCP stream plus whether it ends at a proxy (gh
/// #145): the flag becomes hyper-util's `Connected::proxy`, which is
/// the only thing that flips a request between origin and absolute
/// form. HTTPS tunnels report unproxied — inside TLS the client
/// talks to the origin.
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
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl hyper::rt::Write for ProxyStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

impl hyper_util::client::legacy::connect::Connection for ProxyStream {
    fn connected(&self) -> hyper_util::client::legacy::connect::Connected {
        hyper_util::client::legacy::connect::Connected::new().proxy(self.proxied)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[test]
    fn proxy_variables_parse_with_curl_defaults() {
        assert_eq!(
            parse_proxy("proxy.example:8080"),
            Some(ProxyEndpoint {
                host: "proxy.example".into(),
                port: 8080,
                auth: None,
            })
        );
        assert_eq!(
            parse_proxy("http://user:pass@proxy.example:3128"),
            Some(ProxyEndpoint {
                host: "proxy.example".into(),
                port: 3128,
                auth: Some("user:pass".into()),
            })
        );
        // No port: curl's 1080.
        assert_eq!(parse_proxy("http://proxy.example").unwrap().port, 1080);
        assert_eq!(parse_proxy(""), None);
        assert_eq!(parse_proxy("socks5://proxy.example:1080"), None);
    }

    #[test]
    fn no_proxy_matches_hosts_subdomains_ports_and_star() {
        let list = split_list("example.com, 127.0.0.1:8080, .internal".into());
        assert!(bypasses(&["*".into()], "anything.example", 443));
        assert!(bypasses(&list, "example.com", 80));
        assert!(bypasses(&list, "api.example.com", 443));
        assert!(!bypasses(&list, "notexample.com", 80));
        assert!(bypasses(&list, "127.0.0.1", 8080));
        assert!(!bypasses(&list, "127.0.0.1", 9090));
        assert!(bypasses(&list, "svc.internal", 80));
        assert!(!bypasses(&[], "example.com", 80));
    }

    #[test]
    fn base64_vectors_match_the_standard() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
    }

    /// A CONNECT proxy in front of a plaintext echo target (gh #145):
    /// the handshake reaches the proxy (with the configured auth), the
    /// tunnel carries bytes both ways. TLS above the tunnel is rustls's
    /// tested job, not this layer's.
    #[tokio::test]
    async fn connect_tunnels_bytes_to_the_target() {
        let echo = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let echo_addr = echo.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut stream, _) = echo.accept().await.expect("accept");
            let (mut read, mut write) = stream.split();
            tokio::io::copy(&mut read, &mut write).await.expect("echo");
        });
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_for_task = seen.clone();
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let proxy_addr = proxy.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut client, _) = proxy.accept().await.expect("accept");
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(client.read_u8().await.expect("read"));
            }
            seen_for_task
                .lock()
                .expect("log")
                .push(String::from_utf8_lossy(&head).into_owned());
            let mut upstream = tokio::net::TcpStream::connect(echo_addr)
                .await
                .expect("dial");
            client
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await
                .expect("200");
            let (mut client_read, mut client_write) = client.split();
            let (mut up_read, mut up_write) = upstream.split();
            let first = tokio::io::copy(&mut client_read, &mut up_write);
            let second = tokio::io::copy(&mut up_read, &mut client_write);
            let _ = tokio::join!(first, second);
        });

        let proxies = Proxies {
            http: None,
            https: Some(ProxyEndpoint {
                host: "127.0.0.1".into(),
                port: proxy_addr.port(),
                auth: Some("user:pass".into()),
            }),
            no_proxy: Vec::new(),
        };
        let pins: Arc<Mutex<HashMap<String, Vec<SocketAddr>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let mut http = HttpConnector::new_with_resolver(PinnedResolver {
            inner: GaiResolver::new(),
            pins,
        });
        http.enforce_http(false);
        let mut connector = ProxyTcp::new(http, proxies);
        use tower_service::Service as _;
        let uri: hyper::Uri = "https://example.com:443/".parse().expect("uri");
        let mut tunneled = connector
            .call(uri)
            .await
            .expect("CONNECT establishes the tunnel")
            .inner
            .into_inner();
        tunneled.write_all(b"ping").await.expect("write");
        let mut back = [0u8; 4];
        tunneled.read_exact(&mut back).await.expect("echo");
        assert_eq!(&back, b"ping");

        let log = seen.lock().expect("log").join("");
        assert!(
            log.starts_with("CONNECT example.com:443 HTTP/1.1"),
            "the proxy saw the tunnel request: {log:?}"
        );
        assert!(
            log.contains("Proxy-Authorization: Basic dXNlcjpwYXNz"),
            "the configured credentials ride the handshake: {log:?}"
        );
    }
}
