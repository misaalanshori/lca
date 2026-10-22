//! The startup header (gh #131), split from `mod.rs` (the
//! 1,200-line ceiling): version line plus loaded-resource listing,
//! with the `ui.quiet_startup` layers.
use std::path::Path;

use lca_config::Config;
use lca_permissions::GrantStore;
use lca_permissions::SharedPrompt;
use std::sync::{Arc, Mutex};

use super::session_stats;
use lca_core::ExtensionRegistry;
use lca_session::{Session, SessionStore};

/// The startup header lines (gh #131, pi's header shape): the version
/// line, then the loaded resources (context paths that exist, plus
/// extension/skill/template counts). `quiet` follows `ui.quiet_startup`:
/// `"false"` shows all, `"header"` keeps the version line only,
/// `"true"` hides everything. Pure for the test rows.
pub(super) fn startup_header(
    product: &str,
    abi: &str,
    quiet: &str,
    context: &[std::path::PathBuf],
    extensions: usize,
    skills: usize,
    templates: usize,
) -> Vec<String> {
    if quiet == "true" {
        return Vec::new();
    }
    let mut lines = vec![format!("lca {product} (abi {abi})")];
    if quiet == "header" {
        return lines;
    }
    if !context.is_empty() {
        let paths = context
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("context: {paths}"));
    }
    lines.push(format!(
        "{extensions} {}, {skills} {}, {templates} {}",
        plural("extension", extensions),
        plural("skill", skills),
        plural("template", templates),
    ));
    lines
}

/// `1 extension` / `2 extensions`.
fn plural(word: &str, count: usize) -> String {
    if count == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// Claim the terminal before the expensive startup load (gh #400,
/// type-while-loading).
///
/// `Ui::new` resolves the session and loads the extension registry; on this
/// machine that is 17-20 ms and it grows with what is installed. Before this
/// existed the terminal stayed in the shell's line discipline for that whole
/// window, which measured as two defects in a real pane: text typed then was
/// held by the tty and arrived late in one lump after the header, and a quit
/// key was inert - nothing was listening, so `Ctrl+C` did nothing and the
/// startup carried on regardless.
///
/// Returns the claimed terminal. `LCA_STARTUP_STALL_MS` (a test hook, not a
/// product knob) sleeps here so a live check can type into a startup that is
/// genuinely slow: the real window is far too short to type into by hand, and a
/// fix for "the window is too long" has to be demonstrable *in* the window.
pub(super) fn claim_terminal() -> lca_ui::run_early_input::EarlyInput {
    let early = lca_ui::run_early_input::EarlyInput::start();
    #[cfg(not(test))]
    if let Some(ms) = std::env::var("LCA_STARTUP_STALL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms.min(10_000)));
    }
    early
}

/// Load the extension registry: installed first (an installed copy shadows the
/// bundled one), then the native-linked first-party set, then the bundled
/// provider (ADR-0013).
///
/// Moved here from `mod.rs` under the 1,200-line ceiling: gh #400 grew that
/// file past it by claiming the terminal before this call.
#[allow(clippy::too_many_arguments)] // one more explicit than a regroup: every arg is used once, at one call site.
pub(super) fn load_registry(
    cwd: &Path,
    config: &Config,
    shared_prompt: SharedPrompt,
    shared_dialogs: lca_permissions::SharedDialogs,
    grants: &Arc<Mutex<GrantStore>>,
    store: &Arc<SessionStore>,
    session_cell: &Arc<Mutex<Session>>,
    temp: &Path,
    flags: &crate::CliFlags,
) -> (
    ExtensionRegistry,
    Arc<lca_mcp::manager::McpManager>,
    Vec<String>,
) {
    // The one assembly, shared with headless mode and `--list-models`
    // (gh #8): only the stats source differs, and a session's own reads
    // the session this interface is showing.
    let stats_store = store.clone();
    let stats_session = session_cell.clone();
    let (registry, manager, warnings) = crate::registry::assemble(
        cwd,
        config,
        shared_prompt,
        shared_dialogs,
        grants,
        Arc::new(move || {
            let session = stats_session
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            session_stats(&stats_store, &session)
        }),
        temp,
        flags,
    );
    (registry, manager, warnings)
}

#[cfg(test)]
mod header_tests {
    use super::startup_header;

    fn paths(names: &[&str]) -> Vec<std::path::PathBuf> {
        names.iter().map(std::path::PathBuf::from).collect()
    }

    // Verifies: gh #131 - the header lists AGENTS.md paths when present,
    // with the resource counts behind them.
    #[test]
    fn the_header_lists_context_paths_and_counts() {
        let lines = startup_header(
            "0.6.0",
            "0.6",
            "false",
            &paths(&["/home/u/.lca/AGENTS.md", "/w/.lca/AGENTS.md"]),
            3,
            2,
            1,
        );
        let text = lines.join("\n");
        assert!(
            text.contains("lca 0.6.0") && text.contains("abi 0.6"),
            "version line first: {text}"
        );
        assert!(
            text.contains("/home/u/.lca/AGENTS.md") && text.contains("/w/.lca/AGENTS.md"),
            "both context paths list: {text}"
        );
        assert!(
            text.contains("3 extensions")
                && text.contains("2 skills")
                && text.contains("1 template"),
            "counts follow: {text}"
        );
    }

    // Verifies: gh #131 - quietStartup hides in layers: "header" keeps
    // the version line only, true hides everything.
    #[test]
    fn quiet_startup_hides_in_layers() {
        let context = paths(&["/home/u/.lca/AGENTS.md"]);
        let header = startup_header("0.6.0", "0.6", "header", &context, 1, 1, 1).join("\n");
        assert!(header.contains("lca 0.6.0"), "version stays: {header}");
        assert!(!header.contains("AGENTS.md"), "resources hide: {header}");
        assert!(
            startup_header("0.6.0", "0.6", "true", &context, 1, 1, 1).is_empty(),
            "true hides everything"
        );
        assert!(
            startup_header("0.6.0", "0.6", "false", &[], 0, 0, 0)
                .join("\n")
                .contains("lca 0.6.0"),
            "an empty resource set still headers"
        );
    }
}
