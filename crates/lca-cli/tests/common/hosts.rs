//! Loopback mock hosts for the receipts that need a server: a scripted HTTP host,
//! its OAuth-flavoured variant, and the `127.0.0.2`-or-`localhost` address probe.
//!
//! Split out of `common/mod.rs` when the harness crossed gate 11's ceiling. These
//! are the only helpers in the harness that bind a socket, so they read as one
//! subject: NFR-22's "the only server is a loopback mock the test started".

/// One loopback host serving scripted JSON by path, on an address that the
/// platform will actually bind.
pub struct ScriptedHost {
    /// `http://<host>:<port>`
    pub base: String,
    /// Paths this host was asked for, in order.
    pub paths: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}
/// The second loopback host to use.
///
/// macOS binds `127.0.0.1` and `localhost` only (`EADDRNOTAVAIL` for
/// `127.0.0.2`), so the candidate list is probed rather than assumed.
pub fn second_loopback_host() -> &'static str {
    for candidate in ["127.0.0.2", "localhost"] {
        if std::net::TcpListener::bind((candidate, 0)).is_ok() {
            return candidate;
        }
    }
    "localhost"
}

/// Serve `answers(path) -> (status, body)` on one host.
pub fn scripted_host(
    ip: &str,
    answers: impl Fn(&str) -> (u16, String) + Send + Sync + 'static,
) -> ScriptedHost {
    let listener = std::net::TcpListener::bind((ip, 0)).expect("bind");
    let addr = listener.local_addr().expect("addr");
    // The requested host *name*, not the resolved address: on macOS
    // `localhost` resolves to `::1`, and a base built from `addr` would
    // collapse the two hosts into one.
    let base = format!("http://{ip}:{}", addr.port());
    serve(listener, base, answers)
}

/// [`scripted_host`] on an address the caller already bound, with the base URL
/// the caller already chose (an identity provider must name itself in its own
/// metadata).
pub fn scripted_host_on(
    addr: std::net::SocketAddr,
    base: &str,
    answers: impl Fn(&str) -> (u16, String) + Send + Sync + 'static,
) -> ScriptedHost {
    let ip = base
        .trim_start_matches("http://")
        .rsplit_once(':')
        .map(|(host, _)| host.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let listener = std::net::TcpListener::bind(addr).expect("bind");
    let base = base.to_string();
    let host = ScriptedHost {
        base: base.clone(),
        paths: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
    };
    let recorded = host.paths.clone();
    serve_with(listener, recorded, answers);
    let _ = ip;
    host
}

/// Serve `answers` on an already-bound listener, recording each path.
fn serve(
    listener: std::net::TcpListener,
    base: String,
    answers: impl Fn(&str) -> (u16, String) + Send + Sync + 'static,
) -> ScriptedHost {
    let paths: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = paths.clone();
    serve_with(listener, recorded.clone(), answers);
    ScriptedHost { base, paths }
}

/// The challenge header a 401 carries, when the row needs one.
///
/// Set once per test binary: the scripted host answers statically, and the
/// login row is the only consumer of a `WWW-Authenticate`.
pub static CHALLENGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn serve_with(
    listener: std::net::TcpListener,
    recorded: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    answers: impl Fn(&str) -> (u16, String) + Send + Sync + 'static,
) {
    use std::io::{Read, Write};
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buffer = vec![0u8; 64 * 1024];
            let read = match stream.read(&mut buffer) {
                Ok(read) if read > 0 => read,
                _ => continue,
            };
            let text = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let head = text.split("\r\n\r\n").next().unwrap_or(&text);
            let path = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            recorded.lock().expect("record").push(path.clone());
            let (status, payload) = answers(&path);
            let reason = if status == 200 { "OK" } else { "Error" };
            let mut response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                payload.len()
            );
            // A 401 carries the challenge the sign-in follows (gh #418
            // Phase D's threading); a 200 never does.
            if status == 401
                && let Some(challenge) = CHALLENGE.get()
            {
                response.push_str(&format!("WWW-Authenticate: {challenge}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&payload);
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
}

/// An identity provider answering RFC 9728, RFC 8414, registration, and the
/// token endpoint on its own host.
pub fn oauth_idp_host() -> ScriptedHost {
    let host = second_loopback_host();
    // Bind first so the metadata can name this host: an authorization server
    // that named a *third* host would just move the problem.
    let listener = std::net::TcpListener::bind((host, 0)).expect("bind");
    let addr = listener.local_addr().expect("addr");
    let base = format!("http://{host}:{}", addr.port());
    let named_base = base.clone();
    drop(listener);
    scripted_host_on(addr, &base, move |path| {
        let named = named_base.as_str();
        if path.contains("oauth-protected-resource") {
            (
                200,
                format!(r#"{{"resource":"{named}/mcp","authorization_servers":["{named}"]}}"#),
            )
        } else if path.contains("oauth-authorization-server") {
            (
                200,
                format!(
                    r#"{{"issuer":"{named}","authorization_endpoint":"{named}/authorize","token_endpoint":"{named}/token","registration_endpoint":"{named}/register"}}"#
                ),
            )
        } else if path.contains("/register") {
            (
                200,
                r#"{"client_id":"cli-client","client_secret":"cli-secret"}"#.to_string(),
            )
        } else if path.contains("/token") {
            (
                200,
                r#"{"access_token":"cli-access","refresh_token":"cli-refresh","expires_in":1800,"scope":"resources:read"}"#
                    .to_string(),
            )
        } else {
            (404, "{}".to_string())
        }
    })
}

/// Write the `mcp.json` an OAuth CLI row needs: one HTTP server whose
/// authorization server is named explicitly by `authServerMetadataUrl`.
///
/// gh #467: discovery has three steps (the configured document, the 401
/// challenge's RFC 9728 pointer, then the MCP origin as its own issuer), and
/// this row used to depend on **which one happened to win** - it reached the
/// third on macOS and the second on Linux. That made it a platform-divergent
/// row, which is how a dead fallback stayed invisible. Naming the document here
/// pins the documented first step, so the row takes the same path on every OS.
///
/// The 401 still carries its challenge (the row's scope merge reads it); the
/// configured document simply wins, which is the documented precedence.
pub fn write_oauth_mcp_config(home: &std::path::Path, url: &str, idp_base: &str) {
    let path = home.join(".lca/mcp.json");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &path,
        format!(
            r#"{{"mcpServers": {{"remote": {{"url": "{url}", "oauth": {{"authServerMetadataUrl": "{idp_base}/.well-known/oauth-authorization-server"}}}}}}}}"#
        ),
    )
    .expect("write mcp.json");
}
