//! The daily background update check (FR-CFG-6): at most once a day,
//! never on the startup path, and in headless mode no request at all
//! unless the user switched the option on. The request's answer is
//! only ever parsed as a release tag and shown as text - nothing here
//! installs anything (release policy, "Update checks").

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

/// The window between checks.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// The stamp file's name inside the data directory; its mtime is the stamp.
const STAMP: &str = "last-update-check";
/// Where a newer release is asked about (the pipeline publishes there).
const RELEASES: &str = "https://api.github.com/repos/misaalanshori/lca/releases/latest";

/// Whether another check is allowed yet: no stamp or a day-old stamp
/// is due, anything fresher is not, and a stamp in the future never
/// grants a second check - the requirement says "at most", so clock
/// skew fails closed rather than buying an extra request.
pub fn due(last: Option<SystemTime>, now: SystemTime) -> bool {
    match last {
        None => true,
        Some(last) => now
            .duration_since(last)
            .map(|age| age >= DAY)
            .unwrap_or(false),
    }
}

/// A three-part version out of whatever tag shape a release carries:
/// `0.1.0`, `v0.1.0`, and the repo's decorated `phase5-0.1.0` all
/// parse. Anything else is incomparable, which the caller turns into
/// "not newer".
pub fn semver_of(text: &str) -> Option<(u64, u64, u64)> {
    let text = text.trim();
    // Decorated tags carry the semver after the last dash; when there
    // is no dash, the whole string is the candidate.
    let core = text
        .rsplit('-')
        .find(|part| part.contains('.'))
        .unwrap_or(text);
    let mut parts = core.trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// True when `candidate` names a strictly newer release than
/// `current`. A side that does not parse answers no: a check that
/// cannot compare must not nag.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (semver_of(candidate), semver_of(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// The last check's stamp: the file's modification time, if it exists.
pub fn read_last(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir.join(STAMP)).ok()?.modified().ok()
}

/// Record that today's attempt is being made. Called before the
/// request, so a hanging request cannot become a second one.
pub fn write_last(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(STAMP), b"daily check ran\n")
}

/// Start today's check when it is enabled and due. The stamp is laid
/// down first, then the request runs on a background task, so nothing
/// here blocks the caller (FR-CFG-6: the check never holds up
/// startup). `notify` is the status line's cell in interactive mode;
/// a headless run that explicitly enabled the option reports on
/// stderr instead. Failures stay silent - a background nicety must
/// not surface as an error during someone's session.
pub fn spawn(enabled: bool, notify: Option<Arc<OnceLock<String>>>) {
    if !enabled {
        return;
    }
    let dir = crate::data_dir();
    if !due(read_last(&dir), SystemTime::now()) {
        return;
    }
    // A stamp failure degrades to "checked anyway" rather than
    // silence: an unwritable data directory is already a bigger
    // problem than one extra request.
    write_last(&dir).ok();

    let _background = tokio::spawn(async move {
        let body =
            match tokio::time::timeout(Duration::from_secs(5), lca_registry::plain_get(RELEASES))
                .await
            {
                Ok(Ok(body)) => body,
                _ => return, // offline, slow, or refusing: say nothing
            };
        let tag = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("tag_name")
                    .and_then(|tag| tag.as_str())
                    .map(str::to_owned)
            });
        let Some(tag) = tag else { return };
        if !is_newer(&tag, env!("CARGO_PKG_VERSION")) {
            return;
        }
        // The tag arrives from the network: it goes through the
        // display choke point (FR-UI-2) and stays short, because the
        // status line has one row to spend.
        let tag = lca_tui::sanitize_text(&tag);
        let tag: String = tag.chars().take(48).collect();
        let notice = format!("update available: lca {tag}");
        match notify {
            Some(cell) => {
                cell.set(notice).ok();
            }
            None => eprintln!("{notice}"),
        }
    });
}
