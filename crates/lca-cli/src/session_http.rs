//! The session's inbound HTTP listener (gh #363, ADR-0049): one per process,
//! started when a session owns the port and stopped when it ends.
//!
//! `lca-tools` owns the listener itself — the binding, the token, the routing,
//! the `Host` check. This module is the *lifecycle*: which extensions are
//! mounted, under which session id, and the degradation the brief requires when
//! no port is free.
//!
//! Three decisions live here rather than in the listener:
//!
//! - **One listener per process, shared by every granted extension.** A second
//!   listener would be a second port to tell the user about and a second accept
//!   loop for no gain, because path routing already separates extensions.
//! - **No free port degrades, never crashes.** A web UI is a convenience; the
//!   agent must not fail to start because a port collision happened. The
//!   capability reports unavailable and the session continues.
//! - **The token is shown, not logged.** [`SessionHttp::notice`] is the one
//!   renderer, and it is what the interface displays; nothing here writes the
//!   token to a file, a session record, or a diagnostic stream.

use std::sync::{Arc, Mutex};

use lca_protocol::http::ServeHttpNamespace;
use lca_tools::serve_http::{self, Listener, SessionToken};

/// Collect the mounts from a registry's handles and start the session's
/// listener (gh #363, ADR-0049).
///
/// Lives here rather than inline in `tui/mod.rs` because that file is at the
/// 1,200-line ceiling and because this is the whole lifecycle in one place: the
/// caller gets a started-or-degraded listener plus the notice to show, and
/// nothing else has to know how the two are related.
#[must_use]
pub fn start_for_session(registry: &lca_core::ExtensionRegistry, session: &str) -> SessionHttp {
    let mounts: Vec<std::sync::Arc<dyn lca_protocol::http::ServeHttpNamespace>> = registry
        .handles()
        .iter()
        .filter_map(|handle| handle.serve_http_mount(session))
        .collect();
    SessionHttp::start(mounts)
}

/// Why the listener is not running, when it is not.
///
/// Both variants are *degradations*: the session runs either way, and the
/// difference is what the interface should say so a user who expected a web
/// view knows why there isn't one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// No extension declared `serve-http`, so no listener was started at all.
    /// This is the normal case and deserves no message.
    NothingToServe,
    /// Every port in the fallback range was taken, or the bind failed.
    NoPort(String),
    /// The platform had no random source for a token. A predictable token
    /// would be worse than no listener, so this degrades rather than falls
    /// back to a counter.
    NoToken(String),
}

/// The running listener and the notice that tells the user how to reach it.
pub struct SessionHttp {
    listener: Option<Listener>,
    unavailable: Option<Unavailable>,
}

impl std::fmt::Debug for SessionHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately not `derive`: `Listener`'s own `Debug` redacts the
        // token, and a derived one here would be a second place to get wrong.
        f.debug_struct("SessionHttp")
            .field("running", &self.listener.is_some())
            .field("unavailable", &self.unavailable)
            .finish()
    }
}

impl SessionHttp {
    /// Start the session's listener, mounting every extension that declared
    /// `serve-http`.
    ///
    /// Never returns an error: a listener that cannot start is a capability
    /// that reports unavailable (ADR-0049's decision 5). A caller that wants
    /// the reason reads [`SessionHttp::unavailable`].
    /// The session id is carried by each mount (`WasmExtension::serve_http_mount`
    /// builds it), so it is not a parameter here: a listener that could disagree
    /// with its mounts about the session would route to a namespace that does
    /// not exist.
    #[must_use]
    pub fn start(mounts: Vec<Arc<dyn ServeHttpNamespace>>) -> SessionHttp {
        if mounts.is_empty() {
            return SessionHttp {
                listener: None,
                unavailable: Some(Unavailable::NothingToServe),
            };
        }
        let token = match SessionToken::generate() {
            Ok(token) => token,
            Err(err) => {
                return SessionHttp {
                    listener: None,
                    unavailable: Some(Unavailable::NoToken(err.to_string())),
                };
            }
        };
        let namespaces: Arc<Mutex<Vec<Arc<dyn ServeHttpNamespace>>>> = Arc::new(Mutex::new(mounts));
        match serve_http::bind(namespaces, token) {
            Ok(listener) => SessionHttp {
                listener: Some(listener),
                unavailable: None,
            },
            Err(err) => SessionHttp {
                listener: None,
                unavailable: Some(Unavailable::NoPort(err.to_string())),
            },
        }
    }

    /// Start with nothing mounted, which is how a session with no `serve-http`
    /// extension begins: the same value [`SessionHttp::start`] returns for an
    /// empty mount list.
    #[must_use]
    pub fn inert() -> SessionHttp {
        SessionHttp::start(Vec::new())
    }

    /// The listener, when it is running.
    #[must_use]
    pub fn listener(&self) -> Option<&Listener> {
        self.listener.as_ref()
    }

    /// Why it is not running, when it is not.
    #[must_use]
    pub fn unavailable(&self) -> Option<&Unavailable> {
        self.unavailable.as_ref()
    }

    /// The line the interface shows at session start, or `None` when there is
    /// nothing to say.
    ///
    /// `NothingToServe` is silent on purpose: a session with no web-serving
    /// extension is the normal case, and a notice about a capability nobody
    /// asked for is noise. The other two are degradations a user would
    /// otherwise be unable to explain.
    ///
    /// The URL carries the token because a browser navigation cannot set an
    /// `Authorization` header. This is the only place the token is rendered.
    #[must_use]
    pub fn notice(&self) -> Option<String> {
        match (&self.listener, &self.unavailable) {
            (Some(listener), _) => Some(format!(
                "Web interface on {} - any program on this machine can reach it",
                listener.browser_url()
            )),
            (None, Some(Unavailable::NoPort(reason))) => Some(format!(
                "Web interface unavailable: {reason} (the session continues)"
            )),
            (None, Some(Unavailable::NoToken(reason))) => Some(format!(
                "Web interface unavailable: {reason} (the session continues)"
            )),
            (None, Some(Unavailable::NothingToServe)) | (None, None) => None,
        }
    }

    /// Stop the listener. Idempotent, and also runs on drop, so a session
    /// that exits leaves no bound port.
    pub fn stop(&mut self) {
        if let Some(listener) = self.listener.as_mut() {
            listener.stop();
        }
        self.listener = None;
    }
}

impl Drop for SessionHttp {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lca_protocol::CapabilityError;
    use lca_protocol::http::{HttpRequest, HttpResponse};

    struct Echo;
    impl ServeHttpNamespace for Echo {
        fn extension(&self) -> &str {
            "echo"
        }
        fn session(&self) -> &str {
            "s"
        }
        fn answer(
            &self,
            _base_url: &str,
            _request: HttpRequest,
        ) -> Result<HttpResponse, CapabilityError> {
            Ok(HttpResponse::text(200, "ok"))
        }
        fn record_refusal(&self, _parameter: &str, _reason: &str) {}
    }

    #[test]
    fn a_session_with_no_serving_extension_says_nothing_and_binds_nothing() {
        // The normal case: no notice, no listener, no port. A message here
        // would be a notice about a capability nobody asked for.
        let http = SessionHttp::inert();
        assert!(http.listener().is_none());
        assert_eq!(http.unavailable(), Some(&Unavailable::NothingToServe));
        assert!(http.notice().is_none());
    }

    #[test]
    fn a_started_listener_renders_one_notice_naming_the_token_and_the_port() {
        let http = SessionHttp::start(vec![Arc::new(Echo) as Arc<dyn ServeHttpNamespace>]);
        let listener = http.listener().expect("the listener started");
        let notice = http.notice().expect("a running listener has a notice");
        assert!(notice.contains("127.0.0.1"), "got {notice}");
        assert!(
            notice.contains(listener.token().expose()),
            "the notice must carry the token a browser needs: {notice}"
        );
        // And the honesty the brief asks for: "on this machine" is not
        // "private to LCA".
        assert!(
            notice.contains("any program on this machine"),
            "got {notice}"
        );
    }

    #[test]
    fn the_notice_never_leaks_into_a_debug_rendering() {
        let http = SessionHttp::start(vec![Arc::new(Echo) as Arc<dyn ServeHttpNamespace>]);
        let token = http
            .listener()
            .expect("started")
            .token()
            .expose()
            .to_string();
        let rendered = format!("{http:?}");
        assert!(
            !rendered.contains(&token),
            "debug leaked the token: {rendered}"
        );
    }

    #[test]
    fn stopping_is_idempotent_and_frees_the_port() {
        let mut http = SessionHttp::start(vec![Arc::new(Echo) as Arc<dyn ServeHttpNamespace>]);
        let port = http.listener().expect("started").bound().addr.port();
        http.stop();
        http.stop();
        assert!(http.listener().is_none());
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let mut rebound = None;
        for _ in 0..50 {
            if let Ok(listener) = std::net::TcpListener::bind(addr) {
                rebound = Some(listener);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(rebound.is_some(), "port {port} stayed bound after stop");
    }
}
