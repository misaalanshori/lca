//! Subscriber wiring (gh #258 §5): the two rotating files behind
//! domain filters, plus `LCA_LOG`. Compiled only with `debug-trace`;
//! without it there is nothing to initialize (events vanish pre-init
//! by `tracing`'s own no-subscriber rule).

use std::io::Write;
use std::sync::{Arc, Mutex, MutexGuard};

use tracing_subscriber::layer::Filter;
use tracing_subscriber::layer::Layer;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::logfiles::{DebugPaths, RotatingWriter};

/// Default directives for the internal file.
pub const INTERNAL_FILTER: &str =
    "lca_core=debug,lca_session=debug,lca_permissions=debug,lca_registry=debug";
/// Default directives for the wire file.
pub const WIRE_FILTER: &str = "lca_net=debug,lca_wasm=debug";

/// What to initialize: the file pair plus an optional explicit filter
/// (a `--debug` flag or `LCA_LOG` value; `None` keeps the defaults).
pub struct DebugConfig {
    /// The two files.
    pub paths: DebugPaths,
    /// Explicit `tracing-subscriber` directives, or `None` for defaults.
    pub filter: Option<String>,
}

/// Subscriber setup failure.
#[derive(Debug)]
pub enum InitError {
    /// A debug file could not be opened.
    Io(std::io::Error),
    /// The process already traces somewhere.
    AlreadySet,
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitError::Io(err) => write!(f, "cannot open a debug log file: {err}"),
            InitError::AlreadySet => write!(f, "debug tracing is already initialized"),
        }
    }
}

impl std::error::Error for InitError {}

/// One rotating file behind a cloneable handle.
#[derive(Clone)]
struct FileWriter {
    inner: Arc<Mutex<RotatingWriter>>,
}

/// A locked file as an `io::Write`.
struct GuardWriter<'a>(MutexGuard<'a, RotatingWriter>);

impl Write for GuardWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl<'a> fmt::MakeWriter<'a> for FileWriter {
    type Writer = GuardWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        GuardWriter(
            self.inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

/// Resolve the effective directives: an explicit filter appends to
/// the defaults (a repeated target takes the user's value), and
/// unparseable input falls back to the default rather than failing
/// startup — a typo must not silence the binary.
fn resolve_filter(explicit: Option<&str>, default: &str) -> EnvFilter {
    match explicit {
        Some(filter) => EnvFilter::try_new(format!("{default},{filter}"))
            .unwrap_or_else(|_| EnvFilter::new(default)),
        None => EnvFilter::new(default),
    }
}

/// One file's gate: only its domains pass, and only when the
/// effective directives enable them. The target list holds under an
/// explicit filter too, so `LCA_LOG=lca_core=trace` never lands core
/// events in the wire file.
fn scoped<S>(
    explicit: Option<&str>,
    default: &str,
    targets: &[&str],
) -> impl Filter<S> + Clone + Send + Sync + 'static
where
    S: tracing::Subscriber,
{
    let env = resolve_filter(explicit, default);
    let targets: Vec<String> = targets.iter().map(|target| target.to_string()).collect();
    tracing_subscriber::filter::dynamic_filter_fn(move |meta: &tracing::Metadata<'_>, cx| {
        let scoped = targets.iter().any(|target| {
            meta.target() == target
                || meta.target().starts_with(&format!("{target}::"))
        });
        scoped && Filter::enabled(&env, meta, cx)
    })
}

/// Build the subscriber: the internal layer writes `debug.log`, the
/// wire layer writes `debug-wire.log`, each seeing only its domains.
pub fn subscriber(config: &DebugConfig) -> std::io::Result<impl tracing::Subscriber + Send + Sync> {
    let log = FileWriter {
        inner: Arc::new(Mutex::new(RotatingWriter::open(&config.paths.log)?)),
    };
    let wire = FileWriter {
        inner: Arc::new(Mutex::new(RotatingWriter::open(&config.paths.wire)?)),
    };
    let internal = fmt::layer()
        .with_ansi(false)
        .with_writer(log)
        .with_filter(scoped(
            config.filter.as_deref(),
            INTERNAL_FILTER,
            &["lca_core", "lca_session", "lca_permissions", "lca_registry"],
        ));
    let wire_layer = fmt::layer()
        .with_ansi(false)
        .with_writer(wire)
        .with_filter(scoped(
            config.filter.as_deref(),
            WIRE_FILTER,
            &["lca_net", "lca_wasm"],
        ));
    Ok(tracing_subscriber::registry()
        .with(internal)
        .with(wire_layer))
}

/// Install the subscriber process-wide. A second call fails (the
/// process already traces somewhere) — the caller keeps the first.
pub fn init(config: &DebugConfig) -> Result<(), InitError> {
    subscriber(config)
        .map_err(InitError::Io)?
        .try_init()
        .map_err(|_| InitError::AlreadySet)
}
