//! The modal's sign-in flow (gh #418 Phase G, §3.4 item 3), split from
//! `manager.rs` for the 1,200-line ceiling.
//!
//! The interactive screen needs three things the one-shot `begin_login` verb does
//! not give it: the **authorization URL while the flow still waits**, a way to
//! **deliver a pasted redirect URL**, and a **cancel** that really stops the
//! listener. All three already exist at the capability layer — the CLI's
//! `lca mcp login` reads `oauth_opened()` for the URL, and
//! `oauth_deliver_manual` is R4's SSH fallback — so this module is the *pairing*,
//! not new plumbing.
//!
//! Cancel goes through `Capabilities::cancel()`, which is the seam the capability
//! engine already polls in `oauth_await` (short slices, because an epoch bump
//! cannot interrupt blocked host code; NFR-21). That is why cancelling here
//! actually stops the listener instead of leaving it to its 600-second deadline.

use std::sync::Mutex;

use super::McpManager;

/// One in-flight interactive sign-in.
///
/// One at a time, because `oauth_deliver_manual` addresses the newest open flow
/// anyway: a second concurrent sign-in would make "the newest" ambiguous, and a
/// modal can only show one screen.
#[derive(Debug)]
pub(super) struct SignInFlow {
    /// The server being signed in to.
    pub server: String,
    /// Whether the URL has already been taken, so the screen shows it once and a
    /// per-tick poll does not re-read it.
    pub url_taken: bool,
}

impl McpManager {
    /// Start an interactive sign-in and answer `(authorization URL, status)`.
    ///
    /// The status is what the screen shows under the URL: where the credential is
    /// going (`via <server>: authorization host <host>`, gh #418 Phase D), or the
    /// reason a URL has not appeared yet.
    pub fn begin_interactive_sign_in(&self, target: &str) -> (Option<String>, String) {
        let Some(entry) = self.find(target).ok() else {
            return (None, format!("no MCP server {target:?}"));
        };
        let Some(config) = entry.http_config() else {
            return (
                None,
                format!("`{target}` is a stdio server: nothing to sign in to"),
            );
        };
        if config.has_static_auth() {
            return (
                None,
                format!(
                    "`{target}` authenticates with a header, so there is nothing to sign in to"
                ),
            );
        }
        if config.oauth.is_none() {
            return (None, format!("`{target}` configures no OAuth"));
        }
        // Capture the challenge first, so RFC 9728 discovery has the hint a 401
        // supplies (Phase D's threading).
        let caps = self.caps();
        let _ = crate::remote::remember_unauthorized(&caps, &config, &config.url);
        // A previous flow's URL must not read as this one's.
        let seen_before = caps.oauth_opened().len();
        {
            let mut slot = self.signin_flow_lock();
            *slot = Some(SignInFlow {
                server: target.to_string(),
                url_taken: false,
            });
        }
        let caps_for_thread = caps.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let server = target.to_string();
        std::thread::spawn(move || {
            let outcome = crate::sign_in(caps_for_thread, &config);
            let _ = done_tx.send((server, outcome));
        });
        self.adopt_login_task(target, done_rx);
        // Wait a bounded moment for the URL: the screen is useless without it,
        // and "waiting" for a flow that already has one is a worse answer.
        match self.await_sign_in_url(seen_before, std::time::Duration::from_secs(10)) {
            Some(url) => (Some(url), self.sign_in_note(target)),
            None => (
                None,
                "still discovering the authorization server".to_string(),
            ),
        }
    }

    /// Deliver a pasted redirect URL into the waiting flow. Answers the status
    /// line.
    ///
    /// The URL carries a one-time code, so it is parsed and **not** retained: it
    /// is never echoed into the transcript, never logged, and never stored. That
    /// is the whole reason the paste path exists — a user on the far side of an
    /// SSH tunnel has no other way to finish a flow whose loopback callback
    /// cannot fire.
    pub fn submit_redirect(&self, target: &str, redirect: &str) -> String {
        let waiting = self
            .signin_flow_lock()
            .as_ref()
            .is_some_and(|flow| flow.server == target);
        if !waiting {
            return format!("`{target}` has no sign-in in flight");
        }
        let Some((_, query)) = redirect.split_once('?') else {
            return "that does not look like a redirect URL (no `?` query)".to_string();
        };
        let query = query.split('#').next().unwrap_or(query);
        let params = lca_tools::parse_callback_query(query);
        if params.is_empty() {
            return "the pasted URL carried no parameters".to_string();
        }
        match self.caps().oauth_deliver_manual(params) {
            Ok(()) => "redirect delivered; finishing the exchange".to_string(),
            Err(err) => format!("could not deliver the redirect: {err}"),
        }
    }

    /// Abandon the waiting flow.
    ///
    /// `Capabilities::cancel()` stops the loopback listener and wakes
    /// `oauth_await` out of its slice loop, which is what makes `Esc` immediate
    /// rather than a wait for the flow's own timeout.
    pub fn cancel_sign_in(&self, target: &str) {
        self.caps().cancel();
        let mut slot = self.signin_flow_lock();
        if slot.as_ref().is_some_and(|flow| flow.server == target) {
            *slot = None;
        }
    }

    /// Poll for the flow's URL: `Some` the first time it appears.
    ///
    /// Idempotent per flow, so the screen can ask every tick without re-reading a
    /// URL it already shows.
    pub fn poll_sign_in_url(&self, target: &str) -> Option<String> {
        let mut slot = self.signin_flow_lock();
        let flow = slot.as_mut()?;
        if flow.server != target || flow.url_taken {
            return None;
        }
        let url = self.caps().oauth_opened().last().cloned()?;
        flow.url_taken = true;
        Some(url)
    }

    /// Wait (bounded) for a URL that was not there when the flow started.
    ///
    /// `seen_before` is the count taken before spawning, so a stale URL from an
    /// earlier flow is never reported as this one's.
    fn await_sign_in_url(&self, seen_before: usize, budget: std::time::Duration) -> Option<String> {
        let deadline = std::time::Instant::now() + budget;
        loop {
            let opened = self.caps().oauth_opened();
            if let Some(url) = opened.get(seen_before).cloned() {
                if let Some(flow) = self.signin_flow_lock().as_mut() {
                    flow.url_taken = true;
                }
                return Some(url);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// The sign-in screen's "where is this credential going" line.
    fn sign_in_note(&self, target: &str) -> String {
        match self.discovery_note(target) {
            Some(note) => format!("via {note}"),
            None => "waiting for the browser to finish".to_string(),
        }
    }

    /// The flow slot, taken under the manager's own lock.
    fn signin_flow_lock(&self) -> std::sync::MutexGuard<'_, Option<SignInFlow>> {
        self.signin_flow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The manager's flow slot type, for the struct field.
pub(super) type SharedFlow = Mutex<Option<SignInFlow>>;
