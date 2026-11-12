//! Proxy routing for the registry's HTTP client (gh #308): the rule set the `net`
//! capability uses - env read once per client build, uppercase-then-lowercase,
//! `ALL_PROXY` as the fallback for both schemes, curl's `NO_PROXY` tail match, and a
//! `CONNECT` tunnel for https with credentials from the URL userinfo.
//!
//! It lives apart from `http.rs` because these rules are a self-contained parser
//! plus a connector, and reading them should not require reading the OCI token
//! dance. Credentials are never logged.

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
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Read the configuration through an injected variable lookup.
    ///
    /// The seam exists because a test that asserts "`HTTPS_PROXY` reaches the
    /// decision" has only two honest options: mutate the process environment - which
    /// races every other row sharing the binary, since `http_client` reads it too -
    /// or inject the lookup. This is the second. It is not a test-only hook: a future
    /// caller with its own configuration source (a profile file, a flag layer) uses
    /// the same entry point.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Proxies {
        let get = |upper: &str, lower: &str| lookup(upper).or_else(|| lookup(lower));
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

/// Parse one proxy variable (the tests' seam: the inner parser is private, and a
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
