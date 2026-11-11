//! The capability engine's vocabulary and its free helpers: what a refusal
//! (`Denial`) is, what a grant set covers, the browser opener, the resource
//! source and its size caps, the mutex helper, and the path/address helpers the
//! engine calls.
//!
//! Split out of `mod.rs` for gate 11's 1,200-line ceiling (gh #570). The
//! `Capabilities` state and its methods stay in `mod.rs`, so a reader asking
//! *what a refusal is* or *what a grant covers* finds it here without walking
//! the engine.
//!
//! Every item is re-exported from `mod.rs` under the name it had, so no caller
//! or extension path changed.

use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lca_permissions::ScopeGrant;

use super::errors::BrowserError;
use lca_protocol::CapabilityError;

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
    /// The `net` (or `net-local`) capability was declared at all (gh
    /// #302). An empty declared host list is the *normal* shape for an
    /// extension whose hosts arrive by consent - webfetch ships
    /// `[capabilities.net] hosts = []` on purpose - so without this flag
    /// "declared, no hosts yet" is indistinguishable from "never
    /// declared", and the first real request died on the wrong gate with
    /// "no network capability declared".
    pub net_declared: bool,
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
    /// The `tools` capability, when declared (gh #77): call other
    /// tools through the host.
    pub tools: bool,
    /// The `serve-http` capability's parameters (gh #363, ADR-0049), when
    /// declared. The listener is the host's; this is what the extension
    /// claimed, and its `Some(..)` is also "may be mounted on the port".
    pub serve_http: Option<lca_permissions::ServeHttpSettings>,
}

/// A browser launcher override for `Capabilities::set_browser_opener`.
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

/// The most hosts one OAuth flow may hold at once (gh #290 design point 5).
///
/// A metadata document is attacker-influenced input, so an unbounded host list
/// would let "grant the IdP" become "grant whatever the server listed". The
/// number lives here because this is the layer that enforces it;
/// `lca-mcp::oauth::MAX_FLOW_HOSTS` is the documented mirror a caller reads, and
/// a row pins the two equal so neither can drift alone.
pub const MAX_OAUTH_FLOW_HOSTS: usize = 8;

/// Validate a resource path: relative, no `..`, no NUL, no absolute.
pub(crate) fn resource_relative(path: &str) -> Result<String, CapabilityError> {
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
pub(crate) fn resource_under(path: &str, prefix: &str) -> bool {
    prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/"))
}

/// Collect a directory tree's files as `(relative path, size)`.
pub(crate) fn collect_resources(
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
pub(crate) fn resolve_addrs(host: &str, port: u16) -> Result<Vec<IpAddr>, CapabilityError> {
    use std::net::SocketAddr;
    (host, port)
        .to_socket_addrs()
        .map(|iter| iter.map(|addr: SocketAddr| addr.ip()).collect())
        .map_err(|err| CapabilityError::Io(format!("cannot resolve {host}: {err}")))
}
