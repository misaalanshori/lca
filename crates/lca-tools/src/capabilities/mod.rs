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

mod errors;
mod net;
#[cfg(test)]
mod pinned_tests;
#[cfg(test)]
mod query_tests;
mod store;
mod traits;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_acl;

pub use errors::{BrowserError, CompletionError};

/// Lock a mutex, recovering a poisoned guard rather than panicking.
///
/// A panic while another thread held the lock leaves it poisoned; refusing
/// to recover would take the whole agent down with a lock that is still
/// perfectly usable (S3: one poison-tolerant style everywhere).
pub(crate) fn lock<T: ?Sized>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    /// A live sender for the manual-callback fallback (R4): a pasted
    /// callback URL is delivered on this channel, waking `oauth_await`
    /// exactly as the loopback listener would.
    tx: std::sync::mpsc::Sender<Vec<(String, String)>>,
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
    ) -> Result<(String, lca_protocol::Usage), CompletionError>;

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
pub type BrowserOpener = Arc<dyn Fn(&str) -> Result<(), BrowserError> + Send + Sync>;

/// Where an extension's read-only `resources/` bag lives (ADR-0030,
/// ADR-0032). The engine seam is identical for both delivery modes: an
/// installed package reads its files, a compiled-in extension serves an
/// `include_bytes!` table, and a caller with no bag uses `None`.
#[derive(Debug, Clone, Default)]
pub enum ResourceSource {
    /// The extension has no resource bag.
    #[default]
    None,
    /// An installed package's `resources/` directory.
    Dir(PathBuf),
    /// A compiled-in extension's embedded table: `(relative path, bytes)`.
    Embedded(&'static [(&'static str, &'static [u8])]),
}

/// Per-file size cap for one resource (ADR-0030's suggested 1 MB).
pub const RESOURCE_FILE_MAX_BYTES: u64 = 1024 * 1024;
/// Per-call read cap: a hostile package cannot pull the host into memory
/// games. A single read is one file, so it equals the file cap.
pub const RESOURCE_READ_MAX_BYTES: u64 = RESOURCE_FILE_MAX_BYTES;
/// Per-package size cap (ADR-0030's suggested 32 MB), enforced at install.
pub const RESOURCE_PACKAGE_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Per-value cap for one `state` entry (ADR-0030's "few MB").
pub const STATE_VALUE_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Per-namespace cap for the whole `state` bag.
pub const STATE_TOTAL_MAX_BYTES: u64 = 16 * 1024 * 1024;

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
    /// The extension's read-only resource bag (ADR-0030, ADR-0032).
    resources: ResourceSource,
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
            resources: ResourceSource::None,
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
    #[allow(clippy::expect_used)] // startup-fatal: a capability runtime that cannot start leaves no host calls runnable.
    pub(super) fn drive<F: std::future::Future>(future: F) -> F::Output {
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
    pub(super) fn drive_cancellable<F: std::future::Future>(
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
        lock(&self.denials).clone()
    }

    /// Attach the backend behind the `completion` capability (the host
    /// routes to the active provider; ADR-0008's star topology).
    pub fn set_completion(&self, backend: Arc<dyn CompletionBackend>) {
        *lock(&self.completion) = Some(backend);
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
            let slot = lock(&self.completion);
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
    pub(super) fn live_adhoc_net(&self) -> Vec<lca_permissions::NetPattern> {
        let mut patterns = self.grants.adhoc_net.clone();
        let stored = lock(&self.store).net_patterns(&self.project);
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
    pub(super) fn pin(&self, host: &str, addrs: &[IpAddr]) {
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
        lock(&self.oauth_opened).clone()
    }

    /// Redirect URLs `oauth_begin` bound, oldest first.
    pub fn oauth_begun(&self) -> Vec<String> {
        lock(&self.oauth_begun).clone()
    }

    /// Replace the browser launcher `oauth_open` uses (tests record the
    /// URL instead of opening one); `None` restores the platform default.
    pub fn set_browser_opener(&self, opener: Option<BrowserOpener>) {
        *lock(&self.browser_opener) = opener;
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
        lock(&self.denials).len()
    }

    pub(super) fn record(&self, capability: &str, parameter: &str, reason: &str) {
        lock(&self.denials).push(Denial {
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

    pub(super) fn undeclared(&self, capability: &str) -> CapabilityError {
        let err = CapabilityError::NotGranted(format!(
            "the manifest does not declare the {capability} capability"
        ));
        self.record(capability, capability, &err.to_string());
        err
    }

    pub(super) fn refused(
        &self,
        capability: &str,
        parameter: &str,
        err: CapabilityError,
    ) -> CapabilityError {
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
            let mut store = lock(&self.store);
            let mut prompt = lock(&self.prompt);
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
        let mut table = lock(&self.handles);
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
        let mut table = lock(&self.handles);
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stdout, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let reader = match stdout.as_mut() {
            Some(reader) => reader,
            None => {
                let Some(reader) = child.stdout() else {
                    return Err(CapabilityError::Invalid(format!(
                        "handle {handle} has no stdout"
                    )));
                };
                stdout.insert(reader)
            }
        };
        Ok(crate::process::read_up_to(reader, max)?)
    }

    /// Read up to `max` bytes of the child's stderr; `None` at EOF.
    pub fn process_read_stderr(
        &self,
        handle: u32,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut table = lock(&self.handles);
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stderr, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let reader = match stderr.as_mut() {
            Some(reader) => reader,
            None => {
                let Some(reader) = child.stderr() else {
                    return Err(CapabilityError::Invalid(format!(
                        "handle {handle} has no stderr"
                    )));
                };
                stderr.insert(reader)
            }
        };
        Ok(crate::process::read_up_to(reader, max)?)
    }

    /// Write to the child's stdin.
    pub fn process_write_stdin(&self, handle: u32, bytes: &[u8]) -> Result<u64, CapabilityError> {
        let mut table = lock(&self.handles);
        let entry = table
            .entries
            .get_mut(&handle)
            .ok_or_else(|| CapabilityError::NotFound(format!("unknown handle {handle}")))?;
        let HandleEntry::Process { child, stdin, .. } = entry else {
            return Err(CapabilityError::Invalid(format!(
                "handle {handle} is not a process"
            )));
        };
        let writer = match stdin.as_mut() {
            Some(writer) => writer,
            None => {
                let Some(writer) = child.stdin() else {
                    return Err(CapabilityError::Invalid(format!(
                        "handle {handle} has no stdin"
                    )));
                };
                stdin.insert(writer)
            }
        };
        Ok(crate::process::write_all(writer, bytes)?)
    }

    /// Wait for the child; returns its exit code.
    pub fn process_wait(&self, handle: u32) -> Result<i32, CapabilityError> {
        let mut table = lock(&self.handles);
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
        let mut table = lock(&self.handles);
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
            let mut store = lock(&self.store);
            let mut prompt = lock(&self.prompt);
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
        let mut table = lock(&self.handles);
        let id = table.next;
        table.next += 1;
        table.entries.insert(id, HandleEntry::Pty(child));
        Ok(id)
    }

    /// Read up to `max` terminal bytes; `None` when the session ended.
    pub fn pty_read(&self, handle: u32, max: usize) -> Result<Option<Vec<u8>>, CapabilityError> {
        let mut table = lock(&self.handles);
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
        let mut table = lock(&self.handles);
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
        let mut table = lock(&self.handles);
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
        let mut table = lock(&self.handles);
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
        let mut table = lock(&self.handles);
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
}

/// Validate a resource path: relative, no `..`, no NUL, no absolute.
fn resource_relative(path: &str) -> Result<String, CapabilityError> {
    if path.contains('\0') {
        return Err(CapabilityError::Invalid(
            "resource path contains NUL".into(),
        ));
    }
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Err(CapabilityError::Permission(format!(
            "resource path `{path}` is absolute"
        )));
    }
    let mut parts = Vec::new();
    for component in candidate.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            std::path::Component::CurDir => {}
            _ => {
                return Err(CapabilityError::Permission(format!(
                    "resource path `{path}` leaves the extension's resource tree"
                )));
            }
        }
    }
    Ok(parts.join("/"))
}

/// Whether a relative `path` is under a relative `prefix`.
fn resource_under(path: &str, prefix: &str) -> bool {
    prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/"))
}

/// Collect a directory tree's files as `(relative path, size)`.
fn collect_resources(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, u64)>,
) -> Result<(), CapabilityError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = entry.metadata()?;
        if meta.is_dir() {
            collect_resources(root, &path, out)?;
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            out.push((rel, meta.len()));
        }
    }
    Ok(())
}

/// The host's generic User-Agent (V2, ADR-0031): `lca/<version> (<os>;
/// <arch>; abi-<line>)`. Set at the net gate when the caller sets none, so
/// every extension is covered in one place and an endpoint can identify and
/// rate-limit the agent correctly.
pub fn default_user_agent() -> String {
    format!(
        "lca/{} ({}; {}; abi-{})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        lca_ext_abi::ABI_VERSION,
    )
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
