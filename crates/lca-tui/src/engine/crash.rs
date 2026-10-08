//! Crash reports (gh #81, pi's `crash-log.ts` row): the context the
//! host registers and the file the panic hook writes. Split from
//! `terminal.rs` at the file ceiling; behavior unchanged. Plain data
//! only, so the engine keeps its zero-agent-imports boundary.

use std::sync::Mutex;

/// The crash report's context (gh #81): plain data the host registers
/// (version, data dir, loaded extension names). Plain data only, so
/// the engine keeps its zero-agent-imports boundary: no versions, no
/// paths, no registry cross this line except as strings.
#[derive(Debug, Clone, Default)]
pub struct CrashContext {
    /// The product version line for the report's head.
    pub version: String,
    /// The directory that receives `crash-*.log`.
    pub data_dir: std::path::PathBuf,
    /// The loaded extensions pi's crash hints name.
    pub extensions: Vec<String>,
}

static CRASH_CONTEXT: Mutex<Option<CrashContext>> = Mutex::new(None);

/// Register the crash report's context (gh #81): the host calls this
/// at startup (version, data dir) and after extension loads (names).
/// Best effort by design: a missing context means no file, never a
/// second failure inside the hook.
pub fn set_crash_context(context: CrashContext) {
    *CRASH_CONTEXT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(context);
}

/// Refresh the loaded extension names (gh #81): the registry calls
/// this after every assembly, so the file names what was actually
/// loaded, not what startup guessed.
pub fn set_crash_extensions(extensions: Vec<String>) {
    let mut guard = CRASH_CONTEXT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(context) = guard.as_mut() {
        context.extensions = extensions;
    }
}

/// Crash files kept per data directory: a crashing loop must not fill
/// the disk one report at a time.
const CRASH_KEEP: usize = 10;
/// Backtrace frames per report: the faulting frames, not the runtime's
/// autobiography.
const CRASH_FRAMES: usize = 40;

/// Clone the registered context for the hook: the hook's own write
/// happens outside the lock.
pub(super) fn context_clone() -> Option<CrashContext> {
    CRASH_CONTEXT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Write one crash report for `info` under the registered context (gh
/// #81, pi's `crash-log.ts` row): version, extensions, the panic
/// message with its location, and the faulting frames. Rows are
/// identifiers and frames only — no secrets by construction. Every
/// failure is swallowed: the hook must never fail.
pub(super) fn write_crash_log(context: &CrashContext, info: &std::panic::PanicHookInfo<'_>) {
    let report = crash_report(context, info);
    let dir = &context.data_dir;
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let name = format!(
        "crash-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|age| age.as_secs())
            .unwrap_or(0)
    );
    if std::fs::write(dir.join(&name), report.as_bytes()).is_err() {
        return;
    }
    prune_crash_logs(dir);
}

/// The report text, split for the guard: rows are asserted, the file
/// is the hook's business.
fn crash_report(context: &CrashContext, info: &std::panic::PanicHookInfo<'_>) -> String {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(non-string panic payload)".to_string());
    let location = info
        .location()
        .map(|location| location.to_string())
        .unwrap_or_else(|| "(no location)".to_string());
    let frames = std::backtrace::Backtrace::force_capture().to_string();
    let frames: Vec<&str> = frames.lines().take(CRASH_FRAMES).collect();
    format!(
        "lca crash report\nversion: {version}\nextensions ({count}): {extensions}\nlocation: {location}\nmessage: {message}\nframes:\n{frames}\n",
        version = context.version,
        count = context.extensions.len(),
        extensions = context.extensions.join(", "),
        frames = frames.join("\n"),
    )
}

/// Keep the newest `CRASH_KEEP` reports: a crashing loop must not
/// fill the disk one report at a time.
fn prune_crash_logs(dir: &std::path::Path) {
    let mut logs: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("crash-") && name.ends_with(".log"))
                })
                .collect()
        })
        .unwrap_or_default();
    logs.sort();
    while logs.len() > CRASH_KEEP {
        if let Some(oldest) = logs.first() {
            let _ = std::fs::remove_file(oldest);
        }
        logs.remove(0);
    }
}
