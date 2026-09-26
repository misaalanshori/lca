//! The capability engine shared by both delivery modes: the WASM host's
//! import functions and a native-linked extension call the exact same
//! code, which is what makes conformance results identical by
//! construction (ADR-0013, capability catalog).
//!
//! Every refusal records a [`Denial`] so `lca ext info` can show what an
//! extension attempted (FR-EXT-9), and every user-facing decision runs
//! through the same prompt and grant store the model's own commands use
//! (`docs/flows.md`).

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use http_body_util::Full;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::connect::dns::{GaiResolver, Name};
use lca_permissions::{
    Action, GrantStore, PermissionPrompt, Proposals, ScopeGrant, ScopeRoots, authorize,
    is_local_address, normalize_ip,
};
use lca_protocol::CapabilityError;
use tower_service::Service;

/// One recorded attempt: identity, what was tried, and why it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    /// The capability family: `fs`, `process`, or `pty`.
    pub capability: String,
    /// The parameter the guest supplied.
    pub parameter: String,
    /// Why it was refused.
    pub reason: String,
}

/// What one extension's manifest declared (FR-PERM-1), resolved into the
/// grants the engine enforces. Phase5's install flow intersects this with
/// user approval; until then the declared set is the granted set.
#[derive(Debug, Clone, Default)]
pub struct CapabilityGrants {
    /// Approved `fs` scopes and modes.
    pub fs: Vec<ScopeGrant>,
    /// The `fs` capability was declared at all (FR-PERM-1); an empty
    /// declared set is rejected at manifest parse, so this is exactly
    /// "the manifest declared fs".
    pub fs_declared: bool,
    /// The `process` capability was declared.
    pub process: bool,
    /// The `pty` capability was declared.
    pub pty: bool,
    /// `net` patterns: internet hosts, HTTPS only (ADR-0011).
    pub net: Vec<lca_permissions::NetPattern>,
    /// `net-local` patterns: local ranges, HTTP allowed (ADR-0011).
    pub net_local: Vec<lca_permissions::LocalPattern>,
    /// User-attached hosts outside the manifest's vocabulary
    /// (FR-PERM-16); consent text names the exact host.
    pub adhoc_net: Vec<lca_permissions::NetPattern>,
    /// The loopback OAuth flow, when declared (FR-PROV-3).
    pub oauth: Option<lca_permissions::OAuthSettings>,
    /// The credentials capability, when declared (FR-PERM-6): the
    /// namespace is always `self.name`, never guest input.
    pub credentials: bool,
    /// The `completion` capability, when declared (ADR-0015): ask the
    /// host for a response from the active provider.
    pub completion: bool,
}

type HttpClient =
    Client<hyper_rustls::HttpsConnector<HttpConnector<PinnedResolver>>, Full<hyper::body::Bytes>>;

/// A resolver that returns the address [`Capabilities::net_request`] already
/// checked for a hostname, so hyper cannot re-resolve to a different address
/// between the rebinding check and the connect (FR-PERM-13, ADR-0011). A name
/// with no pin (an IP literal, an ad-hoc local name) falls through to the
/// system resolver.
#[derive(Clone)]
struct PinnedResolver {
    inner: GaiResolver,
    pins: Arc<Mutex<HashMap<String, Vec<SocketAddr>>>>,
}

impl Service<Name> for PinnedResolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let pinned = self
            .pins
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&name.as_str().to_ascii_lowercase())
            .cloned();
        if let Some(addrs) = pinned {
            return Box::pin(async move { Ok(addrs.into_iter()) });
        }
        let future = self.inner.call(name);
        Box::pin(async move {
            let addrs = future.await?;
            Ok(addrs.collect::<Vec<_>>().into_iter())
        })
    }
}

/// One in-flight loopback OAuth flow: the receiver half lives here, the
/// listener runs on its own thread (FR-PROV-3; the extension never binds).
struct OAuthFlow {
    rx: Option<std::sync::mpsc::Receiver<Vec<(String, String)>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// The host-mediated model call (capability catalog `completion`): a
/// granted extension asks; the host routes to whichever provider is
/// currently active, which keeps the extension graph a star with the
/// host at the center (ADR-0008, ADR-0015). Synchronous by contract:
/// the callers are blocking regions (ADR-0014), and the backend does
/// its own runtime bridging.
pub trait CompletionBackend: Send + Sync {
    /// One non-streaming completion: the assistant text plus the usage
    /// the host attributes to the session record that caused the call.
    fn complete(
        &self,
        messages: &[lca_protocol::ChatMessage],
    ) -> Result<(String, lca_protocol::Usage), String>;

    /// Usage summed from `complete` calls since the caller last drained
    /// it; the caller writes it onto the session record it is about to
    /// append (capability catalog: spend shows in session cost).
    /// Backends that track nothing return `None`.
    fn take_usage(&self) -> Option<lca_protocol::Usage> {
        None
    }
}

/// The runtime capability calls fall back to when no ambient runtime
/// exists (see [`Capabilities::drive`]).
static SHARED_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

/// Resolve once `flag` is set. `notify_one` stores a permit when no waiter
/// is registered, and the flag is re-checked, so a cancel that lands before
/// the wait registers still returns.
async fn wait_cancelled(flag: &std::sync::atomic::AtomicBool, notify: &tokio::sync::Notify) {
    loop {
        if flag.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let notified = notify.notified();
        if flag.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        notified.await;
    }
}

enum HandleEntry {
    Response {
        status: u16,
        headers: Vec<(String, String)>,
        body: ResponseBody,
    },
    Process {
        child: crate::process::TreeChild,
        stdout: Option<std::process::ChildStdout>,
        stderr: Option<std::process::ChildStderr>,
        stdin: Option<std::process::ChildStdin>,
    },
    Pty(crate::pty::PtyChild),
}

/// The body half of a response: a live stream the reader pulls frames
/// from, the `Busy` placeholder while a reader holds it outside the
/// lock (the catalog's streaming body reader; one reader per handle),
/// or finished. `Failed` keeps a mid-body error visible to later reads.
enum ResponseBody {
    /// Frames arrive as the server sends them; nothing is buffered
    /// beyond the chunk a read is assembling.
    Live(Box<hyper::body::Incoming>),
    /// Another read holds the body right now.
    Busy,
    /// EOF reached (or a failure consumed the stream).
    Finished,
    /// The stream failed after a chunk was already handed back; the
    /// next read reports it (ponytail: a failure lands on the read
    /// after the one that carried the last bytes).
    Failed(String),
}

#[derive(Default)]
struct HandleTable {
    next: u32,
    entries: HashMap<u32, HandleEntry>,
}

/// A browser launcher override for [`Capabilities::set_browser_opener`].
/// The default is the platform launcher (`xdg-open`/`open`/`cmd start`).
pub type BrowserOpener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// The engine: one per loaded extension.
pub struct Capabilities {
    name: String,
    grants: CapabilityGrants,
    roots: ScopeRoots,
    prompt: Arc<Mutex<dyn PermissionPrompt>>,
    store: Arc<Mutex<GrantStore>>,
    project: PathBuf,
    proposals: Option<Proposals>,
    completion: Arc<Mutex<Option<Arc<dyn CompletionBackend>>>>,
    denials: Arc<Mutex<Vec<Denial>>>,
    /// Auth URLs the extension asked to open (host diagnostics; the
    /// provider-flow tests read the state from here).
    oauth_opened: Arc<Mutex<Vec<String>>>,
    /// Redirect URLs `oauth_begin` bound. Both delivery modes' tests read
    /// the flow's URL from here: the guest cannot report it back, and the
    /// native `identity_login` blocks inside `oauth_await`.
    oauth_begun: Arc<Mutex<Vec<String>>>,
    /// Overrides the platform browser launch (tests record the URL instead
    /// of spawning a browser). `None` uses `xdg-open`/`open`/`cmd start`.
    browser_opener: Arc<Mutex<Option<BrowserOpener>>>,
    /// Set when the host cancels this extension's in-flight work. Blocking
    /// host imports poll it (the OAuth callback today) so a cancelled turn
    /// does not wait out their window: an epoch bump cannot interrupt host
    /// code that is already blocked (FR-CONC-1, NFR-21).
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Wakes a blocked `net` wait the moment [`Capabilities::cancel`] fires.
    /// A `Notify`, not a sleep timer: the wait runs on a caller-owned
    /// runtime that the caller may drop mid-request, and a timer on a
    /// shutting-down runtime panics.
    cancelled_notify: Arc<tokio::sync::Notify>,
    handles: Arc<Mutex<HandleTable>>,
    client: HttpClient,
    /// Addresses already checked for a hostname, consulted by
    /// [`PinnedResolver`] so the connect uses the checked address.
    pins: Arc<Mutex<HashMap<String, Vec<SocketAddr>>>>,
    flows: Arc<Mutex<HashMap<u32, OAuthFlow>>>,
    next_flow: std::sync::atomic::AtomicU32,
}

impl Capabilities {
    /// Build the engine for one extension. `roots.private` is the base
    /// directory; each extension gets its own subdirectory inside it
    /// (capability catalog: `private` is per-extension).
    pub fn new(
        name: impl Into<String>,
        grants: CapabilityGrants,
        roots: ScopeRoots,
        prompt: Arc<Mutex<dyn PermissionPrompt>>,
        store: Arc<Mutex<GrantStore>>,
        project: PathBuf,
        proposals: Option<Proposals>,
    ) -> Capabilities {
        let name = name.into();
        let mut roots = roots;
        roots.private = roots.private.join(&name);
        // The per-extension private directory is the one scope the state-
        // directory exclusion sanctions; create it up front so the first
        // read resolves against a real root rather than a dangling path.
        let _ = std::fs::create_dir_all(&roots.private);
        let pins: Arc<Mutex<HashMap<String, Vec<SocketAddr>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let mut http = HttpConnector::new_with_resolver(PinnedResolver {
            inner: GaiResolver::new(),
            pins: pins.clone(),
        });
        // The wrapped connector must accept `https`: hyper-rustls's
        // `build()` clears this on the connector it creates, but
        // `wrap_connector` leaves a caller-supplied one as-is, and the
        // default `enforce_http` rejects the scheme before TLS is even
        // considered.
        http.enforce_http(false);
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .wrap_connector(http);
        Capabilities {
            name,
            grants,
            roots,
            prompt,
            store,
            project,
            proposals,
            completion: Arc::new(Mutex::new(None)),
            denials: Arc::new(Mutex::new(Vec::new())),
            oauth_opened: Arc::new(Mutex::new(Vec::new())),
            oauth_begun: Arc::new(Mutex::new(Vec::new())),
            browser_opener: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancelled_notify: Arc::new(tokio::sync::Notify::new()),
            handles: Arc::new(Mutex::new(HandleTable::default())),
            client: Client::builder(hyper_util::rt::TokioExecutor::new()).build(https),
            pins,
            flows: Arc::new(Mutex::new(HashMap::new())),
            next_flow: std::sync::atomic::AtomicU32::new(1),
        }
    }

    /// Drive a future to completion from a host call. Capability calls
    /// run on a blocking thread (or a plain thread outside any runtime),
    /// never inside a runtime poll, so the ambient-handle path is always
    /// legal there (ADR-0014's blocking-region rule). Outside any
    /// runtime, one process-long shared runtime drives the call: a live
    /// HTTP connection's background task must outlive the call that
    /// opened it, or a streaming body reader finds the connection dead
    /// on its second read (which is exactly what a throwaway runtime per
    /// call did).
    /// ponytail: callers already inside an async poll would panic on
    /// `block_on`; every current caller is inside a blocking region.
    fn drive<F: std::future::Future>(future: F) -> F::Output {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle.block_on(future),
            Err(_) => SHARED_RUNTIME
                .get_or_init(|| {
                    tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(1)
                        .enable_all()
                        .build()
                        .expect("capability runtime")
                })
                .handle()
                .block_on(future),
        }
    }

    /// Drive a future, but return as soon as this extension is cancelled
    /// instead of waiting it out (NFR-21 for `net` waits: a hung request
    /// must return within the polling window). The future is dropped on
    /// cancellation, which aborts the in-flight request.
    fn drive_cancellable<F: std::future::Future>(
        &self,
        future: F,
    ) -> Result<F::Output, CapabilityError> {
        let cancelled = self.cancelled.clone();
        let notify = self.cancelled_notify.clone();
        Self::drive(async move {
            tokio::select! {
                biased;
                () = wait_cancelled(&cancelled, &notify) => {
                    Err(CapabilityError::Io("request cancelled by the user".into()))
                }
                output = future => Ok(output),
            }
        })
    }

    /// The extension's identity.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Every recorded refusal (FR-EXT-9's data).
    pub fn denials(&self) -> Vec<Denial> {
        self.denials.lock().expect("denial lock").clone()
    }

    /// Attach the backend behind the `completion` capability (the host
    /// routes to the active provider; ADR-0008's star topology).
    pub fn set_completion(&self, backend: Arc<dyn CompletionBackend>) {
        *self.completion.lock().expect("completion lock") = Some(backend);
    }

    /// Ask the host for one response (capability catalog `completion`).
    /// Undeclared means permission-denied and recorded (FR-PERM-3);
    /// declared without a backend means there is no provider to ask.
    pub fn complete(
        &self,
        messages: Vec<lca_protocol::ChatMessage>,
    ) -> Result<(String, lca_protocol::Usage), CapabilityError> {
        if !self.grants.completion {
            return Err(self.refused(
                "completion",
                &self.name,
                CapabilityError::NotGranted(
                    "the manifest does not declare the completion capability".to_string(),
                ),
            ));
        }
        let backend = {
            let slot = self.completion.lock().expect("completion lock");
            slot.clone()
        };
        let Some(backend) = backend else {
            return Err(self.refused(
                "completion",
                &self.name,
                CapabilityError::Invalid("no active provider is available".to_string()),
            ));
        };
        backend.complete(&messages).map_err(|detail| {
            let err = CapabilityError::Io(format!("completion failed: {detail}"));
            self.record("completion", &self.name, &err.to_string());
            err
        })
    }

    /// The grant store this engine reads ad hoc grants from, shared so
    /// the consent flow that attaches one mid-session writes to the
    /// same instance the next request consults (FR-PERM-18).
    pub fn grant_store(&self) -> Arc<Mutex<lca_permissions::GrantStore>> {
        self.store.clone()
    }

    /// The ad hoc hosts for this session: the manifest's own plus
    /// whatever the user has attached in the grant store for this
    /// project (FR-PERM-18: an attachment during a session is honored
    /// for subsequent calls without a restart). ponytail: parsed per
    /// request; the set is tiny, cache it if a hot loop ever notices.
    fn live_adhoc_net(&self) -> Vec<lca_permissions::NetPattern> {
        let mut patterns = self.grants.adhoc_net.clone();
        let stored = self
            .store
            .lock()
            .expect("grant lock")
            .net_patterns(&self.project);
        for pattern in stored {
            if let Ok(parsed) = lca_permissions::parse_net_pattern(&pattern)
                && !patterns.contains(&parsed)
            {
                patterns.push(parsed);
            }
        }
        patterns
    }

    /// Remember the addresses checked for `host`, so the HTTP client's
    /// resolver returns exactly these on connect instead of resolving again
    /// (the TOCTOU the rebinding check otherwise has). Ports are filled in by
    /// the connector from the request URI.
    fn pin(&self, host: &str, addrs: &[IpAddr]) {
        let entries: Vec<SocketAddr> = addrs.iter().map(|ip| SocketAddr::new(*ip, 0)).collect();
        self.pins
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(host.to_ascii_lowercase(), entries);
    }

    /// Note a `ui` ask for a region the manifest never declared
    /// (FR-EXT-9's journal; the render export itself is never called,
    /// capability catalog `ui`).
    pub fn note_ui_denial(&self, region: &str) {
        self.record("ui", region, "the manifest does not declare this region");
    }

    /// Every URL this extension asked the host to open, in order: how
    /// a test (or an audit) sees the authorization URL a login built.
    pub fn oauth_opened(&self) -> Vec<String> {
        self.oauth_opened.lock().expect("oauth lock").clone()
    }

    /// Redirect URLs `oauth_begin` bound, oldest first.
    pub fn oauth_begun(&self) -> Vec<String> {
        self.oauth_begun.lock().expect("oauth lock").clone()
    }

    /// Replace the browser launcher `oauth_open` uses (tests record the
    /// URL instead of opening one); `None` restores the platform default.
    pub fn set_browser_opener(&self, opener: Option<BrowserOpener>) {
        *self.browser_opener.lock().expect("opener lock") = opener;
    }

    /// Signal that this extension's in-flight work is cancelled. Blocking
    /// host waits poll this so a cancelled turn returns within the NFR-21
    /// window instead of waiting out their window. Called from the host's
    /// interrupt path (a WASM epoch bump cannot reach a blocked host call).
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // Wake a `net` wait blocked on the notify (the flag alone is
        // polled only by waits that slice their own timeouts). `notify_one`
        // stores a permit when no waiter is registered yet, so a cancel
        // that lands just before the wait registers is not lost.
        self.cancelled_notify.notify_one();
    }

    /// Whether [`Capabilities::cancel`] fired for the current work.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// How many attempts were refused (FR-EXT-9).
    pub fn denial_count(&self) -> usize {
        self.denials.lock().expect("denial lock").len()
    }

    fn record(&self, capability: &str, parameter: &str, reason: &str) {
        self.denials.lock().expect("denial lock").push(Denial {
            capability: capability.to_string(),
            parameter: parameter.to_string(),
            reason: reason.to_string(),
        });
        // FR-EXT-9: `lca ext info` counts these when nothing from this
        // session is running, so each one also lands in the extension's
        // journal - the same path lca-registry::InstallTree::denials_path
        // reads. Best effort: a denial that cannot be journaled is still
        // recorded in memory and still refused.
        let path = self
            .roots
            .state_dir
            .join("extensions")
            .join(&self.name)
            .join("denials.jsonl");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or(0);
        // Built with serde, not string concatenation: a guest-supplied path or
        // host can contain quotes, backslashes, or newlines, and this journal is
        // counted line by line by `ext info`.
        let line = format!(
            "{}\n",
            serde_json::json!({
                "capability": capability,
                "parameter": parameter,
                "reason": reason,
                "ts": now,
            })
        );
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write as _;
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn undeclared(&self, capability: &str) -> CapabilityError {
        let err = CapabilityError::NotGranted(format!(
            "the manifest does not declare the {capability} capability"
        ));
        self.record(capability, capability, &err.to_string());
        err
    }

    fn refused(&self, capability: &str, parameter: &str, err: CapabilityError) -> CapabilityError {
        self.record(capability, parameter, &err.to_string());
        err
    }

    fn resolve(&self, scope: &str, path: &str, write: bool) -> Result<PathBuf, CapabilityError> {
        self.roots
            .resolve(&self.grants.fs, scope, path, write)
            .map_err(|violation| {
                let err = CapabilityError::from(violation.clone());
                self.record("fs", &format!("{scope}:{path}"), &violation.to_string());
                err
            })
    }

    // ------------------------------------------------------------------
    // fs
    // ------------------------------------------------------------------

    /// Read a file inside a granted scope.
    pub fn fs_read(&self, scope: &str, path: &str) -> Result<Vec<u8>, CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, false)?;
        Ok(std::fs::read(resolved)?)
    }

    /// Write a file inside a granted scope, creating its parent.
    pub fn fs_write(&self, scope: &str, path: &str, bytes: &[u8]) -> Result<(), CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, true)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(resolved, bytes)?;
        Ok(())
    }

    /// List a directory inside a granted scope; sorted for determinism.
    pub fn fs_list(&self, scope: &str, path: &str) -> Result<Vec<String>, CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, false)?;
        let mut names: Vec<String> = std::fs::read_dir(resolved)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        Ok(names)
    }

    /// Stat a path inside a granted scope.
    pub fn fs_stat(&self, scope: &str, path: &str) -> Result<(bool, u64), CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        let resolved = self.resolve(scope, path, false)?;
        let meta = std::fs::metadata(resolved)?;
        Ok((meta.is_dir(), meta.len()))
    }

    /// Resolve a granted scope's own directory: the working directory for
    /// spawned programs must be a scope the extension can already see
    /// (capability catalog).
    fn scope_dir(&self, scope: &str) -> Result<PathBuf, CapabilityError> {
        if !self.grants.fs_declared {
            return Err(self.undeclared("fs"));
        }
        self.resolve(scope, ".", false)
    }

    // ------------------------------------------------------------------
    // process
    // ------------------------------------------------------------------

    /// Spawn a program (argv, no shell) in a granted scope, through the
    /// same prompt and grant store the model's commands use.
    pub fn process_spawn(
        &self,
        program: &str,
        args: &[String],
        cwd_scope: &str,
    ) -> Result<u32, CapabilityError> {
        if !self.grants.process {
            return Err(self.undeclared("process"));
        }
        let dir = self.scope_dir(cwd_scope).inspect_err(|err| {
            self.record(
                "process",
                &format!("{program} (in {cwd_scope})"),
                &err.to_string(),
            );
        })?;
        let display = format!("{program} {}", args.join(" "));
        let action = Action::Shell {
            command: display.clone(),
            cwd: dir.clone(),
        };
        let decision = {
            let mut store = self.store.lock().expect("grant store lock");
            let mut prompt = self.prompt.lock().expect("prompt lock");
            authorize(
                &mut store,
                &self.project,
                &action,
                self.proposals.as_ref(),
                &mut *prompt,
            )
        };
        match decision {
            Ok(outcome) if outcome.allowed => {}
            Ok(_) => {
                return Err(self.refused(
                    "process",
                    &display,
                    CapabilityError::Permission(format!("the user declined {display}")),
                ));
            }
            Err(err) => {
                return Err(self.refused(
                    "process",
                    &display,
                    CapabilityError::Io(err.to_string()),
                ));
            }
        }
        let child = crate::process::spawn_direct(program, args, &dir).map_err(|err| {
            let err = CapabilityError::from(err);
            self.record("process", &display, &err.to_string());
            err
        })?;
        let mut table = self.handles.lock().expect("handle lock");
        let id = table.next;
        table.next += 1;
        table.entries.insert(
            id,
            HandleEntry::Process {
                child,
                stdout: None,
                stderr: None,
                stdin: None,
            },
        );
        Ok(id)
    }

    /// Read up to `max` bytes of the child's stdout; `None` at EOF.
    pub fn process_read_stdout(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stdout, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let reader =
            stdout.get_or_insert_with(|| child.stdout().expect("stdout was piped at spawn"));
        Ok(crate::process::read_up_to(reader, max)?)
    }

    /// Read up to `max` bytes of the child's stderr; `None` at EOF.
    pub fn process_read_stderr(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stderr, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let reader =
            stderr.get_or_insert_with(|| child.stderr().expect("stderr was piped at spawn"));
        Ok(crate::process::read_up_to(reader, max)?)
    }

    /// Write to the child's stdin.
    pub fn process_write_stdin(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stdin, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let writer = stdin.get_or_insert_with(|| child.stdin().expect("stdin was piped at spawn"));
        Ok(crate::process::write_all(writer, bytes)?)
    }

    /// Wait for the child; returns its exit code.
    pub fn process_wait(&self, handle: u32) -> Result<i32, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        Ok(child.wait()?)
    }

    /// Kill the child's whole tree and release the handle.
    pub fn process_kill(&self, handle: u32) -> Result<(), CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        match table.entries.remove(&handle) {
            Some(HandleEntry::Process { mut child, .. }) => {
                child.kill_tree();
                Ok(())
            }
            Some(_) => Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            ))),
            None => Err(CapabilityError::NotFound(format!(
                "unknown handle {handle}"
            ))),
        }
    }

    // ------------------------------------------------------------------
    // pty
    // ------------------------------------------------------------------

    /// Spawn a program attached to a new pseudo-terminal (ADR-0016).
    pub fn pty_spawn(
        &self,
        program: &str,
        args: &[String],
        cwd_scope: &str,
        rows: u16,
        cols: u16,
    ) -> Result<u32, CapabilityError> {
        if !self.grants.pty {
            return Err(self.undeclared("pty"));
        }
        if rows == 0 || cols == 0 {
            return Err(CapabilityError::Invalid(
                "terminal dimensions must be non-zero".to_string(),
            ));
        }
        let dir = self.scope_dir(cwd_scope).inspect_err(|err| {
            self.record(
                "pty",
                &format!("{program} (in {cwd_scope})"),
                &err.to_string(),
            );
        })?;
        let display = format!("{program} {}", args.join(" "));
        let action = Action::Shell {
            command: display.clone(),
            cwd: dir.clone(),
        };
        let decision = {
            let mut store = self.store.lock().expect("grant store lock");
            let mut prompt = self.prompt.lock().expect("prompt lock");
            authorize(
                &mut store,
                &self.project,
                &action,
                self.proposals.as_ref(),
                &mut *prompt,
            )
        };
        match decision {
            Ok(outcome) if outcome.allowed => {}
            Ok(_) => {
                return Err(self.refused(
                    "pty",
                    &display,
                    CapabilityError::Permission(format!("the user declined {display}")),
                ));
            }
            Err(err) => {
                return Err(self.refused("pty", &display, CapabilityError::Io(err.to_string())));
            }
        }
        let child =
            crate::pty::PtyChild::spawn(program, args, &dir, rows, cols, &[]).map_err(|err| {
                let err = CapabilityError::from(err);
                self.record("pty", &display, &err.to_string());
                err
            })?;
        let mut table = self.handles.lock().expect("handle lock");
        let id = table.next;
        table.next += 1;
        table.entries.insert(id, HandleEntry::Pty(child));
        Ok(id)
    }

    /// Read up to `max` terminal bytes; `None` when the session ended.
    pub fn pty_read(&self, handle: u32, max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Pty(pty) = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a pty"
            )));
        };
        Ok(pty.read(max)?)
    }

    /// Forward keystrokes to the program.
    pub fn pty_write(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Pty(pty) = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a pty"
            )));
        };
        Ok(pty.write(bytes)?)
    }

    /// Resize the terminal.
    pub fn pty_resize(&self, handle: u32, rows: u16, cols: u16) -> Result<(), CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Pty(pty) = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a pty"
            )));
        };
        Ok(pty.resize(rows, cols)?)
    }

    /// Wait for the program; returns its exit code.
    pub fn pty_wait(&self, handle: u32) -> Result<i32, CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Pty(pty) = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a pty"
            )));
        };
        Ok(pty.wait()?)
    }

    /// End the session and release the handle.
    pub fn pty_kill(&self, handle: u32) -> Result<(), CapabilityError> {
        let mut table = self.handles.lock().expect("handle lock");
        match table.entries.remove(&handle) {
            Some(HandleEntry::Pty(mut pty)) => {
                pty.kill();
                Ok(())
            }
            Some(_) => Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a pty"
            ))),
            None => Err(CapabilityError::NotFound(format!(
                "unknown handle {handle}"
            ))),
        }
    }

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
        for (key, value) in headers {
            let name = hyper::header::HeaderName::from_bytes(key.as_bytes())
                .map_err(|err| CapabilityError::Invalid(format!("bad header name {key}: {err}")))?;
            let value =
                hyper::header::HeaderValue::from_bytes(value.as_bytes()).map_err(|err| {
                    CapabilityError::Invalid(format!("bad header value for {key}: {err}"))
                })?;
            builder = builder.header(name, value);
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

    // ------------------------------------------------------------------
    // credentials (FR-PERM-6, FR-PERM-7, NFR-14)
    // ------------------------------------------------------------------

    fn credential_path(&self) -> Result<PathBuf, CapabilityError> {
        if !self.grants.credentials {
            return Err(self.refused(
                "credentials",
                &self.name,
                CapabilityError::NotGranted(
                    "the manifest does not declare the credentials capability".to_string(),
                ),
            ));
        }
        // The namespace IS the extension identity: never guest input
        // (FR-PERM-6), so a cross-namespace read has no address to take
        // (FR-PERM-7).
        Ok(self
            .roots
            .state_dir
            .join("credentials")
            .join(format!("{}.json", self.name)))
    }

    fn load_credentials(&self, path: &Path) -> Result<serde_json::Value, CapabilityError> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|err| CapabilityError::Io(format!("credential store corrupt: {err}"))),
            // Absence means "no login yet", not an error.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(serde_json::Value::Object(serde_json::Map::new()))
            }
            // Any other read failure must surface: silently treating it as
            // empty would let the next `set` overwrite stored credentials.
            Err(err) => Err(CapabilityError::Io(format!(
                "cannot read the credential store: {err}"
            ))),
        }
    }

    /// Write the namespace's credential file atomically with owner-only
    /// permissions (NFR-14). Set and delete share this path, so both get the
    /// same temp+rename and mode treatment (delete used to write in place,
    /// leaving a file the process created with the default umask).
    fn write_credentials(
        &self,
        path: &Path,
        data: &serde_json::Value,
    ) -> Result<(), CapabilityError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes =
            serde_json::to_vec_pretty(data).map_err(|err| CapabilityError::Io(err.to_string()))?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(windows)]
        {
            // NFR-14 on Windows: the file would otherwise rely on the
            // user-profile directory's inherited ACL. Replace it with an
            // explicit owner-only DACL before the rename publishes the file.
            windows_acl::set_owner_only(&temp).map_err(CapabilityError::Io)?;
        }
        std::fs::rename(&temp, path)?;
        Ok(())
    }

    /// Read one key from this extension's own namespace. Returns `None`
    /// when absent; a denied or undeclared capability is a recorded
    /// permission error (FR-PERM-3), while "no login yet" is `None` so an
    /// extension can check without distinguishing denial from absence
    /// (capability catalog).
    pub fn credentials_get(&self, key: &str) -> Result<Option<String>, CapabilityError> {
        let path = self.credential_path()?;
        let data = self.load_credentials(&path)?;
        Ok(data.get(key).and_then(|v| v.as_str()).map(str::to_string))
    }

    /// Write one key into this extension's own namespace with
    /// owner-only file permissions (NFR-14).
    pub fn credentials_set(&self, key: &str, value: &str) -> Result<(), CapabilityError> {
        let path = self.credential_path()?;
        let mut data = self.load_credentials(&path)?;
        if !data.is_object() {
            data = serde_json::Value::Object(serde_json::Map::new());
        }
        data[key] = serde_json::Value::String(value.to_string());
        self.write_credentials(&path, &data)
    }

    /// Delete one key from this extension's own namespace.
    pub fn credentials_delete(&self, key: &str) -> Result<(), CapabilityError> {
        let path = self.credential_path()?;
        let mut data = self.load_credentials(&path)?;
        if let Some(map) = data.as_object_mut() {
            map.remove(key);
        }
        self.write_credentials(&path, &data)
    }
}

/// Resolve a host:port to addresses with the blocking resolver (capability
/// calls run off the runtime's poll path, ADR-0014).
fn resolve_addrs(host: &str, port: u16) -> Result<Vec<IpAddr>, CapabilityError> {
    use std::net::SocketAddr;
    (host, port)
        .to_socket_addrs()
        .map(|iter| iter.map(|addr: SocketAddr| addr.ip()).collect())
        .map_err(|err| CapabilityError::Io(format!("cannot resolve {host}: {err}")))
}

/// Parse a callback query string into pairs (the extension verifies
/// `state` itself; the host only parses, per `docs/flows.md`).
fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (decode(key), decode(value)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

fn decode(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.bytes();
    while let Some(byte) = chars.next() {
        match byte {
            b'+' => out.push(' '),
            b'%' => {
                let hi = chars.next();
                let lo = chars.next();
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    let nibble = |b: u8| match b {
                        b'0'..=b'9' => b - b'0',
                        b'a'..=b'f' => b - b'a' + 10,
                        b'A'..=b'F' => b - b'A' + 10,
                        _ => 0,
                    };
                    out.push(char::from((nibble(hi) << 4) | nibble(lo)));
                }
            }
            other => out.push(other as char),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The shared capability traits (lca-protocol): native mode's side of
// the provider world's imports
// ---------------------------------------------------------------------------

impl lca_protocol::ProviderCap for Capabilities {
    fn net_request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<u32, CapabilityError> {
        Capabilities::net_request(self, method, url, headers, body)
    }

    fn net_response_status(&self, handle: u32) -> Result<u16, CapabilityError> {
        Capabilities::net_response_status(self, handle)
    }

    fn net_read_body(&self, handle: u32, max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        Capabilities::net_read_body(self, handle, max)
    }

    fn net_close_response(&self, handle: u32) -> Result<(), CapabilityError> {
        Capabilities::net_close_response(self, handle)
    }

    fn credentials_get(&self, key: &str) -> Option<String> {
        // Denial reads as absence (capability catalog): checking for an
        // existing login needs no denial/absence distinction.
        Capabilities::credentials_get(self, key).unwrap_or(None)
    }

    fn credentials_set(&self, key: &str, value: &str) -> Result<(), CapabilityError> {
        Capabilities::credentials_set(self, key, value)
    }

    fn credentials_delete(&self, key: &str) -> Result<(), CapabilityError> {
        Capabilities::credentials_delete(self, key)
    }
}

impl lca_protocol::OauthCap for Capabilities {
    fn oauth_begin(&self, redirect_path: &str) -> Result<(String, u32), CapabilityError> {
        Capabilities::oauth_begin(self, redirect_path)
    }

    fn oauth_open(&self, url: &str) -> Result<(), CapabilityError> {
        Capabilities::oauth_open(self, url)
    }

    fn oauth_await(&self, handle: u32) -> Result<Vec<(String, String)>, CapabilityError> {
        Capabilities::oauth_await(self, handle)
    }

    fn oauth_end(&self, handle: u32) -> Result<(), CapabilityError> {
        Capabilities::oauth_end(self, handle)
    }
}

#[cfg(test)]
mod tests {
    use super::{GaiResolver, Name, PinnedResolver, Service};
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::{Arc, Mutex};

    // Verifies: FR-PERM-13's pinning - a checked host resolves to exactly the
    // addresses that were checked, so hyper cannot re-resolve to a local
    // address between the rebinding check and the connect.
    #[tokio::test]
    async fn a_pinned_host_resolves_to_the_checked_address() {
        let mut pins: HashMap<String, Vec<SocketAddr>> = HashMap::new();
        pins.insert(
            "example.com".to_string(),
            vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
                0,
            )],
        );
        let mut resolver = PinnedResolver {
            inner: GaiResolver::new(),
            pins: Arc::new(Mutex::new(pins)),
        };
        let addrs: Vec<SocketAddr> = resolver
            .call("Example.COM".parse::<Name>().expect("name"))
            .await
            .expect("resolves")
            .collect();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].ip().to_string(), "203.0.113.7");
    }
}

#[cfg(test)]
mod query_tests {
    use super::parse_query;

    #[test]
    fn query_parsing_decodes_and_keeps_empty_values() {
        let params = parse_query("code=abc%20def&state=x+y&flag");
        assert_eq!(
            params,
            vec![
                ("code".to_string(), "abc def".to_string()),
                ("state".to_string(), "x y".to_string()),
                ("flag".to_string(), String::new()),
            ]
        );
    }
}

/// NFR-14 on Windows: replace the credential file's inherited DACL with one
/// explicit entry granting only the current user full control, so the file is
/// owner-only regardless of the parent directory's permissions.
///
/// `unsafe` is required because the Win32 security API is FFI. Every handle
/// and buffer is released on every path, and the SID buffer outlives the ACE
/// that points into it. This module is the credential path's documented
/// exemption from the crate's `deny(unsafe_code)` (the pty module is the
/// other).
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_acl {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW,
        SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetTokenInformation, NO_INHERITANCE,
        PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Apply an owner-only DACL to `path`.
    pub fn set_owner_only(path: &std::path::Path) -> Result<(), String> {
        // SAFETY: the calls below are the documented Win32 sequence for
        // setting a file DACL. `token` is closed and `acl` is freed on every
        // exit; `buffer` (which owns the SID the ACE points at) lives until
        // the end of the block, after `SetEntriesInAclW` has copied the SID.
        unsafe {
            let mut token: HANDLE = ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(format!(
                    "OpenProcessToken failed: {}",
                    std::io::Error::last_os_error()
                ));
            }

            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len);
            if len == 0 {
                CloseHandle(token);
                return Err(format!(
                    "GetTokenInformation size failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut buffer = vec![0u64; len.div_ceil(8) as usize];
            if GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), len, &mut len) == 0
            {
                CloseHandle(token);
                return Err(format!(
                    "GetTokenInformation failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let sid = (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid;

            let entry = EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_ALL_ACCESS,
                grfAccessMode: SET_ACCESS,
                grfInheritance: NO_INHERITANCE,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: ptr::null_mut(),
                    MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_USER,
                    ptstrName: sid.cast::<u16>(),
                },
            };
            let mut acl: *mut ACL = ptr::null_mut();
            let entries = SetEntriesInAclW(1, &entry, ptr::null(), &mut acl);
            CloseHandle(token);
            if entries != 0 {
                return Err(format!("SetEntriesInAclW failed: error {entries}"));
            }

            let wide: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let result = SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null(),
            );
            LocalFree(acl.cast());
            if result != 0 {
                return Err(format!("SetNamedSecurityInfoW failed: error {result}"));
            }
        }
        Ok(())
    }

    /// Whether `path`'s DACL is protected (inheritance disabled). The write
    /// sets `PROTECTED_DACL_SECURITY_INFORMATION`, so this is the cheap,
    /// verifiable half of "the file no longer inherits the directory ACL".
    #[cfg(test)]
    pub fn dacl_is_protected(path: &std::path::Path) -> Result<bool, String> {
        use windows_sys::Win32::Security::Authorization::GetNamedSecurityInfoW;
        use windows_sys::Win32::Security::{SE_DACL_PROTECTED, SECURITY_DESCRIPTOR};

        // SAFETY: GetNamedSecurityInfoW allocates the descriptor with
        // LocalAlloc; LocalFree releases it, and the control field is read
        // before the free.
        unsafe {
            let wide: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let mut sd: *mut core::ffi::c_void = ptr::null_mut();
            let result = GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut sd,
            );
            if result != 0 {
                return Err(format!("GetNamedSecurityInfoW failed: error {result}"));
            }
            let control = (*sd.cast::<SECURITY_DESCRIPTOR>()).Control;
            LocalFree(sd.cast());
            Ok(control & SE_DACL_PROTECTED != 0)
        }
    }
}

#[cfg(all(windows, test))]
mod windows_acl_tests {
    // Verifies: NFR-14 on Windows - the credential path's ACL helper leaves
    // the file with a protected, non-inheriting DACL.
    #[test]
    fn set_owner_only_protects_the_file() {
        let dir = std::env::temp_dir().join(format!("lca-acl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let file = dir.join("cred.json");
        std::fs::write(&file, b"{}").expect("write");
        super::windows_acl::set_owner_only(&file).expect("set owner-only");
        assert!(
            super::windows_acl::dacl_is_protected(&file).expect("query DACL"),
            "the DACL must be protected (inheritance off)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
