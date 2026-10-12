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
        let tag = lca_ui::sanitize_text(&tag);
        let tag: String = tag.chars().take(48).collect();
        // Gh #131: point at the changelog with the news in it - after
        // the update, `/changelog` in the new binary tells the story.
        let notice = format!("update available: lca {tag} (see /changelog after updating)");
        match notify {
            Some(cell) => {
                cell.set(notice).ok();
            }
            None => eprintln!("{notice}"),
        }
    });
}

/// What `lca update` runs (gh #50, pi's `pi update` flags): the plan is
/// pure so every flag combination is a unit row; the jobs below run it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdatePlan {
    /// Report only: no install, no extension writes, no refetch.
    pub check_only: bool,
    /// The binary itself, through the installer path.
    pub this_binary: bool,
    /// Installed extensions (`ext update --all`).
    pub extensions: bool,
    /// Catalog documents (provider/model metadata), refetched.
    pub models: bool,
    /// Reinstall even when no newer release compares.
    pub force: bool,
    /// Print the condensed release notes.
    pub changelog: bool,
}

/// Route the flags to jobs: bare `update` is the binary itself,
/// `--all` adds both catalog jobs, `--check` reports everywhere.
pub fn plan(
    check: bool,
    extensions: bool,
    models: bool,
    all: bool,
    force: bool,
    changelog: bool,
) -> UpdatePlan {
    let scoped = extensions || models || all;
    UpdatePlan {
        check_only: check,
        this_binary: !scoped || all,
        extensions: extensions || all,
        models: models || all,
        force,
        changelog,
    }
}

/// The installer the self job runs (docs/installation.md's one-liner
/// source): fetched fresh, then executed, so verification is the
/// installer's own verify-then-move path, never the binary's.
#[cfg(unix)]
const INSTALLER_URL: &str = "https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh";
/// Windows takes the PowerShell installer the same way.
#[cfg(windows)]
const INSTALLER_URL: &str = "https://raw.githubusercontent.com/misaalanshori/lca/main/install.ps1";

/// The latest release's tag and notes body (the check and the
/// condensed changelog read the same answer).
async fn latest_release() -> Result<(String, String), String> {
    let body = lca_registry::plain_get(RELEASES)
        .await
        .map_err(|err| format!("cannot reach the release feed: {err}"))?;
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| "the release feed is not JSON".to_string())?;
    let tag = value
        .get("tag_name")
        .and_then(|tag| tag.as_str())
        .ok_or_else(|| "the release feed names no tag".to_string())?;
    let notes = value
        .get("body")
        .and_then(|body| body.as_str())
        .unwrap_or_default();
    Ok((tag.to_string(), notes.to_string()))
}

/// The condensed notes (gh #50's condensed-changelog option): the
/// first twelve non-empty lines, capped, for `--changelog`.
pub fn condense_notes(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(12)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(1500)
        .collect()
}

/// Run the plan: check, self, extensions, models, in that order. The
/// install step shells out to the installer (verification stays the
/// installer's verify-then-move path); every other job reuses the
/// in-process paths (`ext update --all`, catalog refetch).
pub async fn run(plan: UpdatePlan) -> i32 {
    let mut code = crate::exit::OK;
    if plan.this_binary
        && let Err(failure) = run_self(plan).await
    {
        eprintln!("error: {failure}");
        code = crate::exit::INTERNAL;
    }
    if plan.extensions {
        code = run_extensions().await.max(code);
    }
    if plan.models {
        code = run_models().await.max(code);
    }
    code
}

async fn run_self(plan: UpdatePlan) -> Result<(), String> {
    let current = env!("CARGO_PKG_VERSION");
    let (tag, body) = latest_release().await?;
    let tag = lca_ui::sanitize_text(&tag);
    if plan.changelog {
        let notes = condense_notes(&body);
        if notes.is_empty() {
            println!("lca {tag}: no notes");
        } else {
            println!("lca {tag}:\n{notes}");
        }
    }
    if plan.check_only {
        if is_newer(&tag, current) {
            println!("update available: lca {current} -> {tag}");
        } else {
            println!("lca {current} is current");
        }
        return Ok(());
    }
    if !plan.force && !is_newer(&tag, current) {
        println!("lca {current} is current");
        return Ok(());
    }
    install_binary().await?;
    Ok(())
}

/// Fetch the installer and execute it with inherited stdio: the
/// `lca <old> -> <new>` line and the checksum verification are the
/// installer's own (gate 10 covers that script, not this caller).
async fn install_binary() -> Result<(), String> {
    let script = lca_registry::plain_get(INSTALLER_URL)
        .await
        .map_err(|err| format!("cannot fetch the installer: {err}"))?;
    let path = std::env::temp_dir().join(format!("lca-update-{}", std::process::id()));
    std::fs::write(&path, &script).map_err(|err| format!("cannot stage the installer: {err}"))?;
    #[cfg(unix)]
    let status = std::process::Command::new("sh")
        .arg(&path)
        .status()
        .map_err(|err| format!("cannot run the installer: {err}"))?;
    #[cfg(windows)]
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&path)
        .status()
        .map_err(|err| format!("cannot run the installer: {err}"))?;
    let _ = std::fs::remove_file(&path);
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "the installer exited {}",
            status
                .code()
                .map_or("?".to_string(), |code| code.to_string())
        ))
    }
}

async fn run_extensions() -> i32 {
    let tree = lca_registry::InstallTree::new(crate::data_dir().join("extensions"));
    crate::ext::update_all(&tree).await
}

async fn run_models() -> i32 {
    // Gh #50: LCA keeps no versioned models catalog on disk (provider
    // lists are live); what refreshes is the catalog documents the
    // provider metadata rides: the official document plus configured
    // customs, refetched past the read cache. This also warms the
    // `/extensions` hub cache.
    let data = crate::data_dir();
    let user_file = data.join("config.toml");
    let customs = match lca_config::Config::load(&lca_config::LoadInput {
        user_file: user_file.is_file().then_some(user_file),
        ..Default::default()
    }) {
        Ok(config) => config.extensions_catalogs().to_vec(),
        Err(err) => {
            eprintln!("error: cannot read the configuration: {err}");
            return crate::exit::INTERNAL;
        }
    };
    let mut sources = vec![lca_registry::CatalogSource {
        id: "official".to_string(),
        name: "Official".to_string(),
        url: Some(crate::tui::OFFICIAL_CATALOG_URL.to_string()),
    }];
    sources.extend(customs.iter().map(|custom| lca_registry::CatalogSource {
        id: custom.id.clone(),
        name: custom.name.clone(),
        url: Some(custom.url.clone()),
    }));
    let mut code = crate::exit::OK;
    for source in &sources {
        let cache = lca_registry::cache_path(&data, &source.id);
        match lca_registry::fetch_catalog(source, &cache, true).await {
            Ok(entries) => println!("{}: {} entries", source.name, entries.len()),
            Err(err) => {
                eprintln!("error: {}: {err}", source.name);
                code = crate::exit::INTERNAL;
            }
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #50 - every flag combination routes to its jobs;
    // bare `update` is the binary itself, `--all` is everything.
    #[test]
    fn update_flags_route_to_their_jobs() {
        assert_eq!(
            plan(false, false, false, false, false, false),
            UpdatePlan {
                check_only: false,
                this_binary: true,
                extensions: false,
                models: false,
                force: false,
                changelog: false,
            }
        );
        assert!(plan(false, true, false, false, false, false).extensions);
        assert!(
            !plan(false, true, false, false, false, false).this_binary,
            "a scoped update leaves the binary alone"
        );
        let all = plan(false, false, false, true, false, false);
        assert!(all.this_binary && all.extensions && all.models);
        let scoped = plan(false, true, true, false, false, false);
        assert!(!scoped.this_binary && scoped.extensions && scoped.models);
        assert!(plan(true, false, false, false, false, false).check_only);
        assert!(plan(false, false, false, false, true, false).force);
        assert!(plan(false, false, false, false, false, true).changelog);
    }

    // Verifies: gh #50 - the condensed notes are the first lines,
    // capped; an empty body says so by being empty.
    #[test]
    fn condensed_notes_take_the_head_and_cap() {
        let body = "\n# 0.7.0\n\n- one\n- two\n";
        assert_eq!(condense_notes(body), "# 0.7.0\n- one\n- two");
        assert!(condense_notes("").is_empty());
        assert!(condense_notes(&"x\n".repeat(2000)).len() <= 1500);
    }
}
