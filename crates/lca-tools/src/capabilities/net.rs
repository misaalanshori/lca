//! Split from `capabilities.rs` (cycle 7, P3). Behaviour unchanged.

use super::*;

impl Capabilities {
    // ------------------------------------------------------------------
    // net / net-local (capability catalog, ADR-0011)
    // ------------------------------------------------------------------

    /// Start an outbound request through the host (the `net` and
    /// `net-local` dispatch rules from the capability catalog, applied
    /// once here): named internet hosts under `net` rules (HTTPS only,
    /// rebinding refused, FR-PERM-13), local names/addresses/cidrs under
    /// `net-local` rules (HTTP allowed, ADR-0011), and user-attached ad
    /// hoc hosts (FR-PERM-16). Every refusal is recorded (FR-PERM-5).
    pub fn net_request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        let has_any = !self.grants.net.is_empty()
            || !self.grants.net_local.is_empty()
            || !self.live_adhoc_net().is_empty();
        if !has_any {
            return Err(self.refused(
                "net",
                url,
                CapabilityError::NotGranted("no network capability declared".to_string()),
            ));
        }
        let uri: hyper::Uri = url.parse().map_err(|err| {
            self.refused(
                "net",
                url,
                CapabilityError::Invalid(format!("bad url: {err}")),
            )
        })?;
        let scheme = uri.scheme_str().unwrap_or("").to_string();
        let host = uri
            .host()
            .ok_or_else(|| {
                self.refused(
                    "net",
                    url,
                    CapabilityError::Invalid("url has no host".into()),
                )
            })?
            .to_ascii_lowercase();
        let port = match uri.port_u16() {
            Some(port) => port,
            None if scheme == "https" => 443,
            None if scheme == "http" => 80,
            None => {
                return Err(self.refused(
                    "net",
                    url,
                    CapabilityError::Invalid("url scheme needs an explicit port".into()),
                ));
            }
        };

        // Tier1: named hosts (`net` grants plus ad hoc attachments).
        let declared_hit = self.grants.net.iter().find(|p| p.matches_host(&host));
        let adhoc = self.live_adhoc_net();
        let adhoc_hit = adhoc.iter().find(|p| p.matches_host(&host));
        if let Some(pattern) = declared_hit.or(adhoc_hit) {
            let is_adhoc = adhoc_hit.is_some();
            let local_named = host
                .parse::<IpAddr>()
                .map(is_local_address)
                .unwrap_or(false)
                || host == "localhost"
                || host.ends_with(".local");
            let adhoc_local = is_adhoc && local_named;
            if scheme != "https" && !adhoc_local {
                return Err(self.refused(
                    "net",
                    url,
                    CapabilityError::Permission(format!(
                        "`net` grants HTTPS only (refused {scheme} to {host})"
                    )),
                ));
            }
            if !adhoc_local && !pattern.matches(&host, port) {
                // The hostname matched but the port did not: FR-PERM-5's
                // deny-and-record for a mismatched target.
                return Err(self.refused(
                    "net",
                    url,
                    CapabilityError::Permission(format!(
                        "port {port} on {host} is not granted by `{}`",
                        // reconstruct the pattern text for the record
                        host
                    )),
                ));
            }
            if !adhoc_local {
                // An ad hoc grant naming a literal local address consented
                // to that address (FR-PERM-16); a hostname grant gets the
                // rebinding check (FR-PERM-13), recorded distinctly.
                let addrs = resolve_addrs(&host, port)?;
                if addrs.iter().any(|addr| is_local_address(*addr)) {
                    return Err(self.refused(
                        "net",
                        url,
                        CapabilityError::Permission(format!(
                            "rebinding: {host} resolves to a local address"
                        )),
                    ));
                }
                // Connect to the address just checked, not a fresh lookup.
                self.pin(&host, &addrs);
            }
            return self.http_exchange(method, url, headers, body);
        }

        // Tier2: local network.
        if scheme != "http" && scheme != "https" {
            return Err(self.refused(
                "net-local",
                url,
                CapabilityError::Permission("net-local grants HTTP or HTTPS only".into()),
            ));
        }
        // Declared-but-unmatched targets are ordinary denials
        // (FR-PERM-5); the recorded capability is whichever family this
        // sandbox declared, so an IP literal with only `net` granted is
        // a `net` denial, not a phantom `net-local` one. Only "no grant
        // family at all" reached the top's NotGranted.
        let capability = if self.grants.net_local.is_empty() {
            "net"
        } else {
            "net-local"
        };
        if self.grants.net_local.iter().any(|p| p.matches_name(&host)) {
            return self.http_exchange(method, url, headers, body);
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            let ip = normalize_ip(ip);
            if self.grants.net_local.iter().any(|p| p.matches_ip(ip))
                || (is_local_address(ip)
                    // An ad hoc grant naming a literal local address
                    // consented to that address (FR-PERM-16): host match
                    // only, exactly like tier1's ad hoc local rule.
                    && self.live_adhoc_net().iter().any(|p| p.matches_host(&host)))
            {
                return self.http_exchange(method, url, headers, body);
            }
            return Err(self.refused(
                capability,
                url,
                CapabilityError::Permission(format!("{ip} matches no granted local range")),
            ));
        }
        if !self.grants.net_local.is_empty() {
            // A name that will not resolve inside the local space is a
            // plain denial, not an I/O failure (FR-PERM-5).
            let addrs = resolve_addrs(&host, port).unwrap_or_default();
            if addrs.is_empty() {
                return Err(self.refused(
                    capability,
                    url,
                    CapabilityError::Permission(format!(
                        "cannot resolve {host} to a granted local address"
                    )),
                ));
            }
            if !addrs.is_empty()
                && addrs
                    .iter()
                    .all(|addr| self.grants.net_local.iter().any(|p| p.matches_ip(*addr)))
            {
                self.pin(&host, &addrs);
                return self.http_exchange(method, url, headers, body);
            }
            return Err(self.refused(
                capability,
                url,
                CapabilityError::Permission(format!(
                    "{host} resolves outside the granted local ranges"
                )),
            ));
        }
        Err(self.refused(
            capability,
            url,
            CapabilityError::Permission(format!("{host}:{port} matches no granted pattern")),
        ))
    }

    /// Execute the HTTP exchange and park the buffered response in the
    /// handle table.
    fn http_exchange(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        let http_method = hyper::Method::from_bytes(method.as_bytes())
            .map_err(|err| CapabilityError::Invalid(format!("bad method {method}: {err}")))?;
        let mut builder = hyper::Request::builder().method(http_method).uri(url);
        let mut has_user_agent = false;
        for (key, value) in headers {
            if key.eq_ignore_ascii_case("user-agent") {
                has_user_agent = true;
            }
            let name = hyper::header::HeaderName::from_bytes(key.as_bytes())
                .map_err(|err| CapabilityError::Invalid(format!("bad header name {key}: {err}")))?;
            let value =
                hyper::header::HeaderValue::from_bytes(value.as_bytes()).map_err(|err| {
                    CapabilityError::Invalid(format!("bad header value for {key}: {err}"))
                })?;
            builder = builder.header(name, value);
        }
        // V2 (ADR-0031): one generic User-Agent at the net gate when the
        // caller sets none. Traffic hygiene, not provider semantics, so it
        // lives here rather than in each extension.
        if !has_user_agent {
            builder = builder.header(hyper::header::USER_AGENT, default_user_agent());
        }
        let request = builder
            .body(Full::new(hyper::body::Bytes::copy_from_slice(
                body.unwrap_or(&[]),
            )))
            .map_err(|err| CapabilityError::Invalid(format!("bad request: {err}")))?;
        let response = match self.drive_cancellable(self.client.request(request)) {
            Ok(Ok(response)) => response,
            Ok(Err(err)) => {
                // hyper's Display stops at "client error (Connect)"; walk the
                // source chain so the actual connect/TLS cause is visible
                // (a pinned-DNS failure and a refused socket look identical
                // otherwise).
                let mut chain = err.to_string();
                let mut source = std::error::Error::source(&err);
                while let Some(cause) = source {
                    chain.push_str(": ");
                    chain.push_str(&cause.to_string());
                    source = cause.source();
                }
                return Err(CapabilityError::Io(format!("request failed: {chain}")));
            }
            Err(cancelled) => return Err(cancelled),
        };
        let status = response.status().as_u16();
        let response_headers = response
            .headers()
            .iter()
            .map(|(key, value)| {
                (
                    key.as_str().to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect::<Vec<_>>();
        // The head arrives before the body does: nothing is buffered
        // here, the reader pulls frames as the server sends them
        // (capability catalog: streaming body reader).
        let mut table = self.handles.lock().expect("handle lock");
        let id = table.next;
        table.next += 1;
        table.entries.insert(
            id,
            HandleEntry::Response {
                status,
                headers: response_headers,
                body: ResponseBody::Live(Box::new(response.into_body())),
            },
        );
        Ok(id)
    }

    /// The response's HTTP status.
    pub fn net_response_status(&self, handle: u32) -> Result<u16, CapabilityError> {
        let table = self.handles.lock().expect("handle lock");
        match table.entries.get(&handle) {
            Some(HandleEntry::Response { status, .. }) => Ok(*status),
            Some(_) => Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a response"
            ))),
            None => Err(CapabilityError::NotFound(format!(
                "unknown handle {handle}"
            ))),
        }
    }

    /// The response's headers, in receive order.
    pub fn net_response_headers(
        &self,
        handle: u32,
    ) -> Result<Vec<(String, String)>, CapabilityError> {
        let table = self.handles.lock().expect("handle lock");
        match table.entries.get(&handle) {
            Some(HandleEntry::Response { headers, .. }) => Ok(headers.clone()),
            Some(_) => Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a response"
            ))),
            None => Err(CapabilityError::NotFound(format!(
                "unknown handle {handle}"
            ))),
        }
    }

    /// Read up to `max` body bytes from the live stream; `None` at EOF.
    /// The body leaves the table while a frame wait is in flight, so a
    /// slow server never holds up the other capabilities' handles; a
    /// second concurrent reader of the same handle is refused.
    pub fn net_read_body(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        use http_body_util::BodyExt as _;
        let max = max.max(1);
        let mut body = {
            let mut table = self.handles.lock().expect("handle lock");
            match table.entries.get_mut(&handle) {
                Some(HandleEntry::Response { body, .. }) => {
                    match std::mem::replace(body, ResponseBody::Busy) {
                        ResponseBody::Live(inner) => inner,
                        previous => {
                            let message = match &previous {
                                ResponseBody::Failed(message) => {
                                    Some(CapabilityError::Io(message.clone()))
                                }
                                _ => None,
                            };
                            *body = previous;
                            return match message {
                                Some(message) => Err(message),
                                None if matches!(body, ResponseBody::Finished) => Ok(None),
                                None => Err(CapabilityError::Invalid(format!(
                                    "handle {handle} is being read already"
                                ))),
                            };
                        }
                    }
                }
                Some(_) => {
                    return Err(CapabilityError::Invalid(format!(
                        "handle {handle} is not a response"
                    )));
                }
                None => {
                    return Err(CapabilityError::NotFound(format!(
                        "unknown handle {handle}"
                    )));
                }
            }
        };

        // ponytail: the catalog fixes no per-read timeout; the OAuth
        // flow's 300-second default is the model, and a config key can
        // replace it when a slow source needs more.
        const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
        let mut collected: Vec<u8> = Vec::new();
        let mut reached_eof = false;
        let mut failure: Option<String> = None;
        loop {
            if collected.len() >= max {
                break;
            }
            let frame = {
                // The timeout is constructed where it is polled: inside
                // the runtime `drive` establishes, never before it.
                let pulled = self.drive_cancellable(async {
                    tokio::time::timeout(READ_TIMEOUT, body.frame()).await
                });
                match pulled {
                    Ok(Ok(Some(Ok(frame)))) => frame,
                    Ok(Ok(Some(Err(err)))) => {
                        failure = Some(format!("reading the response: {err}"));
                        break;
                    }
                    Ok(Ok(None)) => {
                        reached_eof = true;
                        break;
                    }
                    Ok(Err(_)) => {
                        failure = Some("timed out waiting for the response body".to_string());
                        break;
                    }
                    Err(cancelled) => {
                        failure = Some(cancelled.to_string());
                        break;
                    }
                }
            };
            if let Ok(data) = frame.into_data() {
                collected.extend_from_slice(&data);
            }
            // Non-data frames (trailers) carry no body bytes.
        }

        let final_state = match failure {
            None if reached_eof => ResponseBody::Finished,
            None => ResponseBody::Live(body),
            // The stream is dead either way; keep the error for the
            // read that follows, or surface it now if nothing was read.
            Some(message) if collected.is_empty() => {
                return Err(CapabilityError::Io(message.clone()));
            }
            Some(message) => ResponseBody::Failed(message),
        };
        {
            let mut table = self.handles.lock().expect("handle lock");
            if let Some(HandleEntry::Response { body, .. }) = table.entries.get_mut(&handle) {
                *body = final_state;
            }
            // A handle closed mid-read stays closed: dropping the live
            // stream above aborts the connection, which is correct.
        }
        if collected.is_empty() {
            Ok(None)
        } else {
            Ok(Some(collected))
        }
    }

    /// Release the response.
    pub fn net_close_response(&self, handle: u32) -> Result<(), CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        match table.entries.remove(&handle) {
            Some(HandleEntry::Response { .. }) => Ok(()),
            Some(_) => Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a response"
            ))),
            None => Err(CapabilityError::NotFound(format!(
                "unknown handle {handle}"
            ))),
        }
    }

    // ------------------------------------------------------------------
    // oauth (FR-PROV-3, FR-PROV-4)
    // ------------------------------------------------------------------

    /// Start a loopback flow: the listener binds `127.0.0.1` on an
    /// ephemeral port (FR-PROV-4: local interface only) and a thread
    /// serves exactly one callback before handing its parsed parameters
    /// back (FR-PROV-3).
    pub fn oauth_begin(&self, redirect_path: &str) -> Result<(String, u32), CapabilityError> {
        // A new flow starts uncancelled: a cancel from earlier work must not
        // poison this wait (the host's interrupt is what sets it).
        self.cancelled
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let Some(_settings) = self.grants.oauth.clone() else {
            return Err(self.refused(
                "oauth",
                redirect_path,
                CapabilityError::NotGranted("oauth is not declared".into()),
            ));
        };
        if !redirect_path.starts_with('/') {
            return Err(CapabilityError::Invalid(
                "redirect path must start with `/`".into(),
            ));
        }
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|err| {
            self.refused("oauth", redirect_path, CapabilityError::Io(err.to_string()))
        })?;
        let port = listener
            .local_addr()
            .map_err(|err| CapabilityError::Io(err.to_string()))?
            .port();
        let redirect_url = format!("http://127.0.0.1:{port}{redirect_path}");
        let (tx, rx) = std::sync::mpsc::channel::<Vec<(String, String)>>();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        listener
            .set_nonblocking(true)
            .map_err(|err| CapabilityError::Io(err.to_string()))?;
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
            while !stop_for_thread.load(std::sync::atomic::Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Read until a full request line arrives, the peer
                        // goes away, or patience runs out - one read is not
                        // enough: a client can connect and then sit on the
                        // socket for a beat (a descheduled test thread, a
                        // browser's preconnect), and answering the empty
                        // read would close the connection under it and
                        // fail its write with EPIPE, which is exactly how
                        // this flaked on macOS CI. A connection that never
                        // sends a query-carrying request line is a probe or
                        // a stray; it does not consume the flow, the loop
                        // just goes back to accepting.
                        // BSD and Linux disagree about whether an
                        // accepted socket inherits the listener's
                        // nonblocking flag: macOS it does, so the first
                        // read would come back WouldBlock before the
                        // client had typed, get treated as "peer gone",
                        // and close the connection under it. Explicitly
                        // blocking, the read_timeout below is the only
                        // clock in play on both platforms.
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(60)));
                        let mut buf = vec![0u8; 8192];
                        let mut filled = 0usize;
                        let mut target: Option<String> = None;
                        while filled < buf.len() {
                            match stream.read(&mut buf[filled..]) {
                                Ok(0) => break,
                                Ok(n) => {
                                    filled += n;
                                    let text = String::from_utf8_lossy(&buf[..filled]);
                                    if let Some(line) = text.lines().next()
                                        && let Some(t) = line.split_whitespace().nth(1)
                                    {
                                        target = Some(t.to_string());
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        let Some(target) =
                            target.and_then(|t| t.split_once('?').map(|q| q.1.to_string()))
                        else {
                            continue;
                        };
                        let params = parse_query(&target);
                        let page = "HTTP/1.1 200 OK
content-type: text/html
                                    content-length:63
connection: close

                                    <html><body>You can close this tab and return to LCA.</body></html>";
                        let _ = stream.write_all(page.as_bytes());
                        let _ = tx.send(params);
                        return;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(_) => return,
                }
                if std::time::Instant::now() > deadline {
                    return;
                }
            }
        });
        let id = self
            .next_flow
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.flows
            .lock()
            .expect("flow lock")
            .insert(id, OAuthFlow { rx: Some(rx), stop });
        self.oauth_begun
            .lock()
            .expect("oauth lock")
            .push(redirect_url.clone());
        Ok((redirect_url, id))
    }

    /// Open a URL in the user's browser (best effort; the extension falls
    /// back to displaying the URL when this fails).
    pub fn oauth_open(&self, url: &str) -> Result<(), CapabilityError> {
        if !url.starts_with("http://127.0.0.1:") && !url.starts_with("https://") {
            return Err(CapabilityError::Invalid(format!(
                "refusing to open {url}: only the loopback flow or https"
            )));
        }
        self.oauth_opened
            .lock()
            .expect("oauth lock")
            .push(url.to_string());
        if let Some(opener) = self.browser_opener.lock().expect("opener lock").clone() {
            return opener(url).map_err(CapabilityError::Io);
        }
        #[cfg(target_os = "linux")]
        let mut cmd = {
            let mut c = std::process::Command::new("xdg-open");
            c.arg(url);
            c
        };
        #[cfg(target_os = "macos")]
        let mut cmd = {
            let mut c = std::process::Command::new("open");
            c.arg(url);
            c
        };
        #[cfg(windows)]
        let mut cmd = {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "start", "", url]);
            c
        };
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|err| CapabilityError::Io(format!("cannot open a browser: {err}")))?;
        Ok(())
    }

    /// Wait for the flow's callback; returns its parsed query parameters,
    /// a timeout (the catalog's300-second default comes from the manifest;
    /// the per-thread deadline above is the hard ceiling), or a cancellation.
    pub fn oauth_await(&self, handle: u32) -> Result<Vec<(String, String)>, CapabilityError> {
        let timeout = self
            .grants
            .oauth
            .as_ref()
            .map(|settings| settings.timeout_seconds)
            .unwrap_or(300);
        let receiver = {
            let mut flows = self.flows.lock().expect("flow lock");
            flows
                .get_mut(&handle)
                .and_then(|flow| flow.rx.take())
                .ok_or_else(|| CapabilityError::NotFound(format!("unknown oauth flow {handle}")))?
        };
        // Poll in short slices rather than one long receive: a host wait must
        // observe the cancellation flag within NFR-21's window, and the epoch
        // bump that cancels a WASM call cannot interrupt blocked host code.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
        loop {
            if self.is_cancelled() {
                let _ = self.oauth_end(handle);
                return Err(CapabilityError::Io(format!(
                    "oauth flow {handle} cancelled"
                )));
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                let _ = self.oauth_end(handle);
                return Err(CapabilityError::Timeout(format!(
                    "no callback within {timeout}s on flow {handle}"
                )));
            }
            match receiver.recv_timeout(remaining.min(std::time::Duration::from_millis(50))) {
                Ok(params) => return Ok(params),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = self.oauth_end(handle);
                    return Err(CapabilityError::Io(format!(
                        "oauth flow {handle} listener ended"
                    )));
                }
            }
        }
    }

    /// Abandon a flow and stop its listener.
    pub fn oauth_end(&self, handle: u32) -> Result<(), CapabilityError> {
        let flows = self.flows.lock().expect("flow lock");
        match flows.get(&handle) {
            Some(flow) => {
                flow.stop.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
            None => Err(CapabilityError::NotFound(format!(
                "unknown oauth flow {handle}"
            ))),
        }
    }
}
