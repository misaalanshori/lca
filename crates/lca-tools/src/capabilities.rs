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
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use http_body_util::Full;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use lca_permissions::{
    Action, GrantStore, PermissionPrompt, Proposals, ScopeGrant, ScopeRoots, authorize,
    is_local_address, normalize_ip,
};
use lca_protocol::CapabilityError;

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
}

type HttpClient = Client<hyper_rustls::HttpsConnector<HttpConnector>, Full<hyper::body::Bytes>>;

/// One in-flight loopback OAuth flow: the receiver half lives here, the
/// listener runs on its own thread (FR-PROV-3; the extension never binds).
struct OAuthFlow {
    rx: Option<std::sync::mpsc::Receiver<Vec<(String, String)>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// The runtime capability calls fall back to when no ambient runtime
/// exists (see [`Capabilities::drive`]).
static SHARED_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

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

/// The engine: one per loaded extension.
pub struct Capabilities {
    name: String,
    grants: CapabilityGrants,
    roots: ScopeRoots,
    prompt: Arc<Mutex<dyn PermissionPrompt>>,
    store: Arc<Mutex<GrantStore>>,
    project: PathBuf,
    proposals: Option<Proposals>,
    denials: Arc<Mutex<Vec<Denial>>>,
    handles: Arc<Mutex<HandleTable>>,
    client: HttpClient,
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
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();
        Capabilities {
            name,
            grants,
            roots,
            prompt,
            store,
            project,
            proposals,
            denials: Arc::new(Mutex::new(Vec::new())),
            handles: Arc::new(Mutex::new(HandleTable::default())),
            client: Client::builder(hyper_util::rt::TokioExecutor::new()).build(https),
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

    /// The extension's identity.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Every recorded refusal (FR-EXT-9's data).
    pub fn denials(&self) -> Vec<Denial> {
        self.denials.lock().expect("denial lock").clone()
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
            crate::pty::PtyChild::spawn(program, args, &dir, rows, cols).map_err(|err| {
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
            || !self.grants.adhoc_net.is_empty();
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
        let adhoc_hit = self.grants.adhoc_net.iter().find(|p| p.matches_host(&host));
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
        if self.grants.net_local.iter().any(|p| p.matches_name(&host)) {
            return self.http_exchange(method, url, headers, body);
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            let ip = normalize_ip(ip);
            if self.grants.net_local.iter().any(|p| p.matches_ip(ip))
                || (is_local_address(ip)
                    && self.grants.adhoc_net.iter().any(|p| p.matches(&host, port)))
            {
                return self.http_exchange(method, url, headers, body);
            }
            return Err(self.refused(
                "net-local",
                url,
                CapabilityError::Permission(format!("{ip} matches no granted local range")),
            ));
        }
        // Declared-but-unmatched targets are ordinary denials
        // (FR-PERM-5); only "no grant family at all" reached the top's
        // NotGranted.
        let capability = if self.grants.net_local.is_empty() {
            "net"
        } else {
            "net-local"
        };
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
        let response = Self::drive(self.client.request(request))
            .map_err(|err| CapabilityError::Io(format!("request failed: {err}")))?;
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
                let pulled =
                    Self::drive(async { tokio::time::timeout(READ_TIMEOUT, body.frame()).await });
                match pulled {
                    Ok(Some(Ok(frame))) => frame,
                    Ok(Some(Err(err))) => {
                        failure = Some(format!("reading the response: {err}"));
                        break;
                    }
                    Ok(None) => {
                        reached_eof = true;
                        break;
                    }
                    Err(_) => {
                        failure = Some("timed out waiting for the response body".to_string());
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
                        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                        let mut buf = vec![0u8; 8192];
                        let read = stream.read(&mut buf).unwrap_or(0);
                        let request = String::from_utf8_lossy(&buf[..read]).into_owned();
                        let params = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .and_then(|target| target.split_once('?'))
                            .map(|(_, query)| parse_query(query))
                            .unwrap_or_default();
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

    /// Wait for the flow's callback; returns its parsed query parameters
    /// or a timeout (the catalog's300-second default comes from the
    /// manifest; the per-thread deadline above is the hard ceiling).
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
        receiver
            .recv_timeout(std::time::Duration::from_secs(timeout))
            .map_err(|_| {
                let _ = self.oauth_end(handle);
                CapabilityError::Timeout(format!("no callback within {timeout}s on flow {handle}"))
            })
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
        let text = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".to_string());
        serde_json::from_str(&text)
            .map_err(|err| CapabilityError::Io(format!("credential store corrupt: {err}")))
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
        data[key] = serde_json::Value::String(value.to_string());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes =
            serde_json::to_vec_pretty(&data).map_err(|err| CapabilityError::Io(err.to_string()))?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temp, &path)?;
        Ok(())
    }

    /// Delete one key from this extension's own namespace.
    pub fn credentials_delete(&self, key: &str) -> Result<(), CapabilityError> {
        let path = self.credential_path()?;
        let mut data = self.load_credentials(&path)?;
        data.as_object_mut().map(|map| map.remove(key));
        let bytes =
            serde_json::to_vec_pretty(&data).map_err(|err| CapabilityError::Io(err.to_string()))?;
        std::fs::write(&path, bytes)?;
        Ok(())
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
