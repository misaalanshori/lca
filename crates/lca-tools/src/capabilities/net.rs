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
        self.net_request_once(method, url, headers, body, false)
    }

    /// Reach one host **for the current OAuth flow only** (gh #418 Phase D).
    ///
    /// An MCP server's authorization server can live on a host the static
    /// grant list never names: the 401's `resource_metadata` or the RFC 8414
    /// issuer points somewhere else (Auth0, Okta, Sentry). Granting every
    /// such host permanently would widen what ordinary tool traffic can
    /// reach, which is not what the user consented to. So the caller attaches
    /// the discovered host with [`Self::oauth_flow_attach`], runs the flow's
    /// own requests through this method, and the host disappears when the
    /// flow ends.
    ///
    /// The host set is **only** consulted here: [`Self::net_request`] sees
    /// the static grants alone, so a flow host can never carry ordinary
    /// traffic. Loopback `http://` is allowed (RFC 8252, and how a local IdP
    /// is reached); everything else must be `https://`, checked before any
    /// request leaves.
    pub fn oauth_flow_net_request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        self.net_request_with(method, url, headers, body, false, true)
    }

    /// The host patterns reachable for the current flow (empty outside one).
    pub fn oauth_flow_hosts(&self) -> Vec<lca_permissions::NetPattern> {
        lock(&self.oauth_flow_hosts).clone()
    }

    /// Attach the hosts a sign-in will need. Replaces the set: one flow at a
    /// time, and the caller starts from the hosts it discovered.
    pub fn oauth_flow_attach(
        &self,
        hosts: impl IntoIterator<Item = String>,
    ) -> Result<(), CapabilityError> {
        let mut patterns = Vec::new();
        for host in hosts {
            let pattern = lca_permissions::parse_net_pattern(&host).map_err(|err| {
                CapabilityError::Invalid(format!("OAuth host {host:?} is not usable: {err}"))
            })?;
            if !patterns.contains(&pattern) {
                patterns.push(pattern);
            }
        }
        // Extend, not replace: the caller may attach in two waves (the
        // discovery URLs it can see, then the endpoints discovery named).
        // Deduplicated, because the same host across both waves is normal.
        let mut hosts = lock(&self.oauth_flow_hosts);
        for pattern in patterns {
            if !hosts.contains(&pattern) {
                hosts.push(pattern);
            }
        }
        Ok(())
    }

    /// Drop every flow-scoped host. Called when the flow ends, whatever the
    /// outcome.
    pub fn oauth_flow_clear(&self) {
        lock(&self.oauth_flow_hosts).clear();
    }

    /// [`Self::net_request`] for a **user-initiated** call: where a prompt
    /// is reachable, an ungranted host is offered as an ad hoc `net` grant
    /// (gh #302, FR-PERM-16) instead of failing outright, and an approval
    /// re-runs the dispatch once.
    ///
    /// This is deliberately *not* what `net_request` does. Most network
    /// calls inside this host are not the user's intent - `discover_models`
    /// probes an endpoint and falls back to a curated list on any error,
    /// and a provider's login flow has its own consent already. Turning
    /// every one of those into a modal would ask the same question twice
    /// (the second time from a background thread, with no user action
    /// behind it) and would leave a spinner behind a modal nobody expected.
    /// So the ask is a property of the *call*, not of the capability: the
    /// tool path that the model or the user deliberately invoked opts in
    /// here, and everything else keeps the plain deny. The deny remains the
    /// enforcement backstop either way - an approval is what lets the
    /// request out, never the absence of a prompt.
    pub fn net_request_asking(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        // One consent round per request: an approval re-runs the whole
        // dispatch, so a refusal consent cannot fix (a port pin, a
        // rebinding address) still ends in the honest denial instead of
        // looping.
        self.net_request_once(method, url, headers, body, true)
    }

    /// [`Self::net_request`] with the consent budget made explicit: the
    /// retry after an approval passes `false` and can only ever deny.
    fn net_request_once(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
        may_ask: bool,
    ) -> Result<u32, CapabilityError> {
        self.net_request_with(method, url, headers, body, may_ask, false)
    }

    /// The real dispatch. `flow` admits the OAuth flow's own host set (gh
    /// #418 Phase D); every other caller passes `false`, which is what keeps
    /// a discovered host out of ordinary traffic.
    fn net_request_with(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
        may_ask: bool,
        flow: bool,
    ) -> Result<u32, CapabilityError> {
        // Gh #302: "declared with no hosts" is the consent-first shape
        // (webfetch ships exactly that), so declaration is the gate here,
        // not a non-empty host list. A sandbox that never declared `net`
        // or `net-local` still gets the flat NotGranted refusal.
        if !self.grants.net_declared
            && self.grants.net.is_empty()
            && self.grants.net_local.is_empty()
            && self.live_adhoc_net().is_empty()
        {
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

        // Tier1: named hosts (`net` grants plus ad hoc attachments, plus the
        // OAuth flow's own discovered hosts when this *is* the flow).
        let declared_hit = self.grants.net.iter().find(|p| p.matches_host(&host));
        let mut adhoc = self.live_adhoc_net();
        if flow {
            for pattern in self.oauth_flow_hosts() {
                if !adhoc.contains(&pattern) {
                    adhoc.push(pattern);
                }
            }
        }
        let adhoc_hit = adhoc.iter().find(|p| p.matches_host(&host));
        if let Some(pattern) = declared_hit.or(adhoc_hit) {
            let is_adhoc = adhoc_hit.is_some();
            // A bracketed IPv6 literal (`[::1]`, how a URL spells one) has to
            // lose its brackets before it can be parsed as an address; without
            // that, a loopback IdP reached as `http://[::1]:PORT` was refused
            // plain http even though `localhost` was granted (gh #418 Phase D,
            // found by the portable-host fallback in the two-host test).
            let local_named = host
                .trim_start_matches('[')
                .trim_end_matches(']')
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
            // Gh #302: consent covers an ungranted local target too -
            // it is the same "nothing covers this" case as tier 1's tail,
            // reached by a different route.
            if may_ask && self.consent_to_host(&host, port) {
                // The retry keeps the same admission set: an approval must not
                // silently narrow an OAuth flow's reach (gh #418 Phase D).
                return self.net_request_with(method, url, headers, body, false, flow);
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
        // Gh #302: nothing covers this host. Where a prompt is reachable
        // this is where the user gets to attach it (FR-PERM-16) instead of
        // reading a dead end; headless and deny-rule paths answer "no" and
        // fall through to exactly the refusal above.
        if may_ask && self.consent_to_host(&host, port) {
            return self.net_request_once(method, url, headers, body, false);
        }
        Err(self.refused(
            capability,
            url,
            CapabilityError::Permission(format!("{host}:{port} matches no granted pattern")),
        ))
    }

    /// Ask for one host's consent through the permission layer's own
    /// `net` action (gh #302): the same modal, grant store, deny-rule
    /// precedence, yolo answer and session-pattern semantics every other
    /// approval uses, so this builds no second prompt path.
    ///
    /// Returns whether the host is now granted. `false` covers a decline,
    /// an unreachable prompt (headless answers denied), a deny rule, and
    /// a store failure - each of which leaves the caller on the refusal
    /// path with its own honest message.
    fn consent_to_host(&self, host: &str, port: u16) -> bool {
        let action = Action::Net {
            host: self.consent_pattern(host, port),
        };
        let decision = {
            let mut store = lock(&self.store);
            let mut prompt = lock(&self.prompt);
            authorize(
                &mut store,
                &self.project,
                &action,
                self.proposals.as_ref(),
                &mut *prompt,
                None,
            )
        };
        match decision {
            Ok(outcome) => outcome.allowed,
            Err(err) => {
                // A store that cannot persist must not silently become a
                // grant: refuse and let the caller report the denial.
                // No tracing dependency in this crate: the refusal the
                // caller reports carries the store failure's own text.
                let _ = &err;
                false
            }
        }
    }

    /// The ad hoc pattern that will actually cover this request
    /// (ADR-0022's vocabulary): the bare host only when the default
    /// 443 path applies, otherwise `host:port`. A local name needs no
    /// port because the ad hoc local branch in the tier-1 rule matches
    /// on the host alone (FR-PERM-16's literal-address shape).
    fn consent_pattern(&self, host: &str, port: u16) -> String {
        let local_named = host
            .parse::<IpAddr>()
            .map(is_local_address)
            .unwrap_or(false)
            || host == "localhost"
            || host.ends_with(".local");
        if local_named || port == 443 {
            host.to_string()
        } else {
            format!("{host}:{port}")
        }
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
        log_http_request(method, url, headers, body);
        let started = std::time::Instant::now();
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
        log_http_response(status, started.elapsed().as_millis(), &response_headers);
        // The head arrives before the body does: nothing is buffered
        // here, the reader pulls frames as the server sends them
        // (capability catalog: streaming body reader).
        let mut table = lock(&self.handles);
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
        let table = lock(&self.handles);
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
        let table = lock(&self.handles);
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
            let mut table = lock(&self.handles);
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
                // Return as soon as a data frame arrives so a streaming
                // body (SSE) is delivered incrementally rather than
                // buffered until `max` bytes or EOF (FR-CORE-4's
                // first-token latency).
                if !collected.is_empty() {
                    break;
                }
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
            let mut table = lock(&self.handles);
            if let Some(HandleEntry::Response { body, .. }) = table.entries.get_mut(&handle) {
                *body = final_state;
            }
            // A handle closed mid-read stays closed: dropping the live
            // stream above aborts the connection, which is correct.
        }
        if collected.is_empty() {
            Ok(None)
        } else {
            log_response_body_chunk(handle, &collected);
            Ok(Some(collected))
        }
    }

    /// Release the response.
    pub fn net_close_response(&self, handle: u32) -> Result<(), CapabilityError> {
        let mut table = lock(&self.handles);
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
        // A second sender so the manual-callback fallback can wake the wait
        // without the loopback listener (R4).
        let tx_for_manual = tx.clone();
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
                        let params = super::parse_callback_query(&target);
                        let page = super::loopback_oauth_page();
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
        lock(&self.flows).insert(
            id,
            OAuthFlow {
                rx: Some(rx),
                stop,
                tx: tx_for_manual,
            },
        );
        lock(&self.oauth_begun).push(redirect_url.clone());
        Ok((redirect_url, id))
    }

    /// Open a URL in the user's browser (best effort; the extension falls
    /// back to displaying the URL when this fails).
    pub fn oauth_open(&self, url: &str) -> Result<(), CapabilityError> {
        // A loopback address, not the literal `127.0.0.1` string: an
        // authorization server can be `127.0.0.2`, `localhost`, or `::1`, and
        // refusing those made a legitimate local IdP unopenable (gh #418
        // Phase D).
        let loopback_http = url
            .strip_prefix("http://")
            .and_then(|rest| rest.split('/').next())
            .map(|authority| {
                let host_port = authority.rsplit('@').next().unwrap_or(authority);
                let host = if host_port.starts_with('[') {
                    host_port
                        .split_once(']')
                        .map(|(inside, _)| format!("{inside}]"))
                        .unwrap_or_else(|| host_port.to_string())
                } else {
                    host_port.split(':').next().unwrap_or(host_port).to_string()
                };
                let bare = host.trim_start_matches('[').trim_end_matches(']');
                bare.eq_ignore_ascii_case("localhost")
                    || bare.ends_with(".localhost")
                    || bare
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            })
            .unwrap_or(false);
        if !loopback_http && !url.starts_with("https://") {
            return Err(CapabilityError::Invalid(format!(
                "refusing to open {url}: only a loopback flow or https"
            )));
        }
        lock(&self.oauth_opened).push(url.to_string());
        if let Some(opener) = lock(&self.browser_opener).clone() {
            return opener(url).map_err(|err| CapabilityError::Io(err.to_string()));
        }
        // R3: the shared shell-free launcher. The old inline Windows path
        // was `cmd /C start "" <url>`, whose parser split the authorize URL
        // at the first `&`; see `crate::open`.
        crate::open_url(url).map_err(CapabilityError::Io)
    }

    /// Wait for the flow's callback; returns its parsed query parameters,
    /// a timeout (the catalog's300-second default comes from the manifest;
    /// the per-thread deadline above is the hard ceiling), or a cancellation.
    pub fn oauth_await(&self, handle: u32) -> Result<Vec<(String, String)>, CapabilityError> {
        let timeout = lock(&self.oauth_timeout).unwrap_or_else(|| {
            self.grants
                .oauth
                .as_ref()
                .map(|settings| settings.timeout_seconds)
                .unwrap_or(300)
        });
        let receiver = {
            let mut flows = lock(&self.flows);
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
        let flows = lock(&self.flows);
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

    /// Deliver a manually pasted OAuth callback to the flow awaiting one
    /// (R4's manual fallback, pi's `acquireAuthCode`). The pasted redirect
    /// URL's query parameters wake [`Self::oauth_await`] exactly as the
    /// loopback listener would, so a browser that cannot reach the
    /// loopback (or a failed auto-open) still completes the login. The
    /// most recent open flow is the one waiting (one login at a time).
    pub fn oauth_deliver_manual(
        &self,
        params: Vec<(String, String)>,
    ) -> Result<(), CapabilityError> {
        let flows = lock(&self.flows);
        let Some((id, flow)) = flows.iter().max_by_key(|(id, _)| **id) else {
            return Err(CapabilityError::NotFound(
                "no oauth flow is open".to_string(),
            ));
        };
        let id = *id;
        flow.tx
            .send(params)
            .map_err(|_| CapabilityError::NotFound(format!("oauth flow {id} is no longer waiting")))
    }
}

#[cfg(feature = "debug-trace")]
fn truncate_chars(s: &str, max_chars: usize) -> (&str, usize) {
    if let Some((idx, _)) = s.char_indices().nth(max_chars) {
        (&s[..idx], s.len() - idx)
    } else {
        (s, 0)
    }
}

fn log_http_request(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>) {
    #[cfg(feature = "debug-trace")]
    {
        let redacted_url = lca_debug::redact::redact_url(url);
        let header_lines = headers
            .iter()
            .map(|(k, v)| lca_debug::redact::redact_header(k, v))
            .collect::<Vec<_>>()
            .join("; ");
        lca_debug::net::event(
            "http.request",
            &format!(
                "{method} {redacted_url} headers=[{header_lines}] body_bytes={}",
                body.map(|b| b.len()).unwrap_or(0)
            ),
        );
        if let Some(b) = body
            && !b.is_empty()
        {
            let text = String::from_utf8_lossy(b);
            let redacted = lca_debug::redact::redact_json(&text);
            let (head, extra) = truncate_chars(&redacted, 4096);
            let preview = if extra > 0 {
                format!("{head}... [+{extra} bytes]")
            } else {
                head.to_string()
            };
            lca_debug::net::event("http.request.body", &preview);
        }
    }
    #[cfg(not(feature = "debug-trace"))]
    {
        let _ = (method, url, headers, body);
    }
}

fn log_http_response(status: u16, elapsed_ms: u128, headers: &[(String, String)]) {
    #[cfg(feature = "debug-trace")]
    {
        let resp_header_lines = headers
            .iter()
            .map(|(k, v)| lca_debug::redact::redact_header(k, v))
            .collect::<Vec<_>>()
            .join("; ");
        lca_debug::net::event(
            "http.response",
            &format!("status={status} ms={elapsed_ms} headers=[{resp_header_lines}]"),
        );
    }
    #[cfg(not(feature = "debug-trace"))]
    {
        let _ = (status, elapsed_ms, headers);
    }
}

fn log_response_body_chunk(handle: u32, chunk: &[u8]) {
    #[cfg(feature = "debug-trace")]
    {
        if chunk.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(chunk);
        let redacted = if text.trim_start().starts_with('{') {
            lca_debug::redact::redact_json(&text)
        } else {
            text.to_string()
        };
        let (head, extra) = truncate_chars(&redacted, 2048);
        let preview = if extra > 0 {
            format!("{head}... [+{extra} bytes]")
        } else {
            head.to_string()
        };
        lca_debug::net::event("http.response.body", &format!("handle={handle} {preview}"));
    }
    #[cfg(not(feature = "debug-trace"))]
    {
        let _ = (handle, chunk);
    }
}
