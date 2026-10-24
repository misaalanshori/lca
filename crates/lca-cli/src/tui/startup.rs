//! The startup header (gh #131), split from `mod.rs` (the
//! 1,200-line ceiling): version line plus loaded-resource listing,
//! with the `ui.quiet_startup` layers.
use std::path::Path;

use lca_config::Config;
use lca_permissions::GrantStore;
use lca_permissions::SharedPrompt;
use std::sync::{Arc, Mutex};

use super::session_stats;
use super::tmux_doctor;
use lca_core::ExtensionRegistry;
use lca_session::{Session, SessionStore};

/// The startup header lines (gh #131, pi's header shape): the version
/// line, then the loaded resources (context paths that exist, plus
/// extension/skill/template counts). `quiet` follows `ui.quiet_startup`:
/// `"false"` shows all, `"header"` keeps the version line only,
/// `"true"` hides everything. Pure for the test rows.
pub fn startup_header(
    product: &str,
    abi: &str,
    quiet: &str,
    context: &[std::path::PathBuf],
    extensions: usize,
    skills: usize,
    templates: usize,
) -> Vec<String> {
    startup_header_with(
        product,
        abi,
        quiet,
        context,
        extensions,
        skills,
        templates,
        &lca_ui::CollisionSummary::default(),
    )
}

// ---------------------------------------------------------------------------
// Gh #400 gap 2: the compact header and its expand.
// ---------------------------------------------------------------------------

/// The one-line header a default launch shows (gh #400 gap 2).
///
/// pi's shape: the version, the counts, and the key that reveals the rest. The
/// context paths and the per-section resource rows live behind
/// [`expanded_header`] - a launch should not spend six rows on facts a reader
/// consults once.
///
/// Collisions are **not** part of this line. They ride `startup_header_with`,
/// which every launch uses, because a collision survives `quietStartup` and must
/// therefore sit outside the compact/expanded decision entirely.
pub fn compact_header(
    product: &str,
    abi: &str,
    _context: &[std::path::PathBuf],
    extensions: usize,
    skills: usize,
    templates: usize,
    _collisions: lca_ui::CollisionSummary,
) -> Vec<String> {
    vec![format!(
        "lca {product} (abi {abi})\u{2003}{extensions} {}, {skills} {}, {templates} {} \u{2502} {} for details",
        plural("extension", extensions),
        plural("skill", skills),
        plural("template", templates),
        lca_tui::engine::keybindings::KeybindingsManager::new().display_keys("app.tools.expand"),
    )]
}

/// The long-form header, shown when the reader expands the compact line
/// (gh #400 gap 2): the version, the context sources, one row per resource
/// section, and the collision notices.
///
/// **Sections with nothing in them render zero rows** - no divider over an empty
/// list, no blank spacer where a section would be. That is the §4 "Section Loom"
/// rule, and it is what makes the table read as a table rather than as a
/// half-drawn one.
pub fn expanded_header(
    product: &str,
    abi: &str,
    context: &[std::path::PathBuf],
    extensions: usize,
    skills: usize,
    templates: usize,
    collisions: lca_ui::CollisionSummary,
) -> Vec<String> {
    expanded_header_fitted(
        product,
        abi,
        context,
        extensions,
        skills,
        templates,
        collisions,
        u16::MAX,
    )
}

/// [`expanded_header`] with a width budget: every row goes through the one fitter
/// the transcript uses (gh #400 gap 2 / §4's "Column Gauge").
///
/// A row that is too long is truncated with an ellipsis rather than clipped, so a
/// deep AGENTS.md path reads as shortened instead of as a different path.
#[allow(clippy::too_many_arguments)] // eight launch facts, each used once; a struct here would be a type with one constructor.
pub fn expanded_header_fitted(
    product: &str,
    abi: &str,
    context: &[std::path::PathBuf],
    extensions: usize,
    skills: usize,
    templates: usize,
    collisions: lca_ui::CollisionSummary,
    width: u16,
) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    rows.push(fit_row(&format!("lca {product} (abi {abi})"), width));
    if let Some(paths) = context_row(context) {
        rows.push(fit_row(&paths, width));
    }
    let resources = resources_row(extensions, skills, templates);
    if !resources.is_empty() {
        rows.push(fit_row(&resources, width));
    }
    for row in collision_rows(&collisions) {
        rows.push(fit_row(&row, width));
    }
    rows
}

/// The `context:` row, or `None` when no context file loaded.
///
/// `None` rather than an empty string: the caller pushes nothing, so an absent
/// section cannot leave a blank row behind.
fn context_row(context: &[std::path::PathBuf]) -> Option<String> {
    if context.is_empty() {
        return None;
    }
    let paths = context
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("context: {paths}"))
}

/// The resource-count row for the sections that have something in them.
///
/// Only the non-zero sections are named: `0 skills` is noise in a table whose job
/// is to tell the reader what *is* loaded. The compact line keeps all three
/// counts, because there the point is the shape of the launch.
fn resources_row(extensions: usize, skills: usize, templates: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    if extensions > 0 {
        parts.push(format!("{extensions} {}", plural("extension", extensions)));
    }
    if skills > 0 {
        parts.push(format!("{skills} {}", plural("skill", skills)));
    }
    if templates > 0 {
        parts.push(format!("{templates} {}", plural("template", templates)));
    }
    parts.join(", ")
}

/// Project the registry's collision reports into the header's row shape
/// (gh #400 gap 3).
///
/// The conversion lives here rather than in `lca-ui` because `CollisionReport` is
/// `lca-core`'s and `lca-ui` does not depend on it - the same split
/// `GenerationUs` uses (ADR-0036). Nothing is re-derived: the loser is the
/// report's own `extension`, the winner is its `winner` field, so the notice can
/// never disagree with what actually loaded.
pub fn collision_summary(reports: &[lca_core::CollisionReport]) -> lca_ui::CollisionSummary {
    lca_ui::CollisionSummary::from_rows(
        reports
            .iter()
            .map(|report| lca_ui::CollisionEntry {
                name: report.name.clone(),
                kind: report.kind.to_string(),
                winner: report.winner.clone(),
                loser: report.extension.clone(),
            })
            .collect(),
    )
}

/// One notice per collision (gh #400 gap 3): the name, a ✓ beside the side that
/// kept it, a ✗ beside the side that was skipped, and the word `skipped` so the
/// marks are not the only signal (NFR-28: never colour or glyph alone).
///
/// Empty in, empty out - a launch with no collision prints nothing.
pub fn collision_rows(summary: &lca_ui::CollisionSummary) -> Vec<String> {
    summary
        .entries
        .iter()
        .map(|entry| {
            format!(
                "{kind} \"{name}\" collision: \u{2713} {winner} / \u{2717} {loser} (skipped)",
                kind = entry.kind,
                name = entry.name,
                winner = entry.winner,
                loser = entry.loser,
            )
        })
        .collect()
}

/// Fit one row to `width` cells, measuring **visible** width so an ANSI sequence or
/// a double-width glyph cannot stagger the frame (gh #400 gap 2 / §4).
pub fn fit_row(row: &str, width: u16) -> String {
    let budget = usize::from(width);
    if lca_tui::engine::text::visible_width(row) <= budget {
        return row.to_string();
    }
    lca_tui::engine::text::truncate_to_width(row, budget, "\u{2026}", false)
}

/// The full startup header for a launch: the compact line, or the expanded table,
/// plus the collisions that are shown in **both** states (gh #400 gaps 2 and 3).
///
/// `quiet` keeps its three layers: `true` hides the header entirely, `header` keeps
/// the version line only, anything else shows the compact form. Collisions are
/// outside that decision on purpose - a shadowed extension is a diagnostic, and a
/// user who chose a quiet startup still needs to learn why the thing they installed
/// is not the thing that loaded.
#[allow(clippy::too_many_arguments)] // the existing signature plus the collision summary: every arg is one fact about the launch.
pub fn startup_header_with(
    product: &str,
    abi: &str,
    quiet: &str,
    context: &[std::path::PathBuf],
    extensions: usize,
    skills: usize,
    templates: usize,
    collisions: &lca_ui::CollisionSummary,
) -> Vec<String> {
    let mut lines = match quiet {
        "true" => Vec::new(),
        "header" => vec![format!("lca {product} (abi {abi})")],
        _ => compact_header(
            product,
            abi,
            context,
            extensions,
            skills,
            templates,
            collisions.clone(),
        ),
    };
    // Diagnostics survive quiet (gh #400 gap 3).
    lines.extend(collision_rows(collisions));
    lines
}

/// The startup header's two prepared states for this launch (gh #400 gaps 2 and 3):
/// the compact line and the table behind `ctrl+o`.
///
/// One function so `mod.rs` does not carry six locals and two calls at its own
/// ceiling - and so "what the header says" is readable in one place, next to the
/// two functions that compose it.
pub fn launch_header(
    data: &std::path::Path,
    cwd: &std::path::Path,
    trusted: bool,
    flags: &crate::CliFlags,
    registry: &lca_core::ExtensionRegistry,
    config: &lca_config::Config,
    tmux_doctor: &tmux_doctor::Cell,
) -> (Vec<String>, Vec<String>) {
    let collisions = collision_summary(registry.collisions());
    let context = crate::prompt::context_sources(data, cwd, trusted, flags.no_context_files);
    let extension_count = registry.registered_names().len();
    let skill_count = lca_tools::skills::collect(&crate::skills_roots(cwd, flags)).len();
    let template_count = lca_tools::prompts::collect(&crate::prompt_roots(cwd)).len();
    let compact = startup_header_with(
        env!("PRODUCT_VERSION"),
        lca_session::ABI_VERSION,
        config.ui_quiet_startup(),
        &context,
        extension_count,
        skill_count,
        template_count,
        &collisions,
    );
    let expanded = expanded_header(
        env!("PRODUCT_VERSION"),
        lca_session::ABI_VERSION,
        &context,
        extension_count,
        skill_count,
        template_count,
        collisions,
    );
    (
        compact,
        doctor_rows_quiet(tmux_doctor, expanded, config.ui_quiet_startup()),
    )
}

/// Append the tmux doctor's notice to the **expanded** header (gh #400 gap 4).
///
/// The expanded section rather than the transcript, on the brief's instruction and
/// for a reason visible in the pandes: the transcript is the conversation, and a
/// diagnostic about the terminal is not part of it - it has no place in scrollback
/// an hour later, and it would push the first message down. The expanded header is
/// where a reader goes to ask "what did this launch see", which is exactly the
/// question the doctor answers.
///
/// Silence when there is no finding, so the healthy case adds **no row at all** -
/// the same rule gap 2's sections follow, and the one that keeps the table from
/// reading as half-drawn.
///
/// A still-running probe is silent too: `notice_from` returns `None` while the
/// thread is out, which is the price of never blocking the first frame. On this
/// machine the load is 6-9x the probe, so in practice the answer is there.
pub fn doctor_rows(tmux_doctor: &tmux_doctor::Cell, rows: Vec<String>) -> Vec<String> {
    doctor_rows_quiet(tmux_doctor, rows, "false")
}

/// [`doctor_rows`] honouring `ui.quiet_startup` (gh #400 gap 4).
///
/// The brief's rule: `quietStartup` suppresses the doctor. That is **not** the
/// collision rule and the difference is deliberate. A collision means the thing
/// the reader installed did not load - they have no other way to learn it, so it
/// rides through quiet. A tmux finding is advice about a chord they have not
/// pressed yet; a reader who asked for quiet has said "do not tell me what I did
/// not ask", and honouring that is what makes the setting worth having. It also
/// keeps the doctor from becoming the one line a quiet startup still prints, which
/// is how a diagnostic turns into noise.
pub fn doctor_rows_quiet(
    tmux_doctor: &tmux_doctor::Cell,
    mut rows: Vec<String>,
    quiet: &str,
) -> Vec<String> {
    if quiet == "true" {
        return rows;
    }
    if let Some(notice) = tmux_doctor::notice_from(tmux_doctor) {
        for line in wrap_notice(&notice) {
            rows.push(fit_row(&line, u16::MAX));
        }
    }
    rows
}

/// Break the notice into rows that read as a paragraph.
///
/// Not word-wrap: the notice is two sentences and the second is the advice, so a
/// row break at the sentence boundary is what makes it scannable. A terminal
/// narrower than a sentence still wraps it in the renderer - this only decides
/// where the *deliberate* break is.
pub fn wrap_notice(notice: &str) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    for sentence in notice.split_inclusive(". ") {
        rows.push(sentence.trim_end().to_string());
    }
    rows
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
pub(super) fn claim_terminal() -> (lca_ui::run_early_input::EarlyInput, tmux_doctor::Cell) {
    let early = lca_ui::run_early_input::EarlyInput::start();
    // Gh #400 gap 4: start the tmux keyboard probe **here**, before the load.
    // One `tmux show` measured 34 ms and a launch asks two, while the load below
    // runs 200-300 ms - so the probe is finished before the header is composed,
    // and nothing on the first-frame path ever waits for it.
    let doctor = tmux_doctor::spawn();
    #[cfg(not(test))]
    if let Some(ms) = std::env::var("LCA_STARTUP_STALL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms.min(10_000)));
    }
    (early, doctor)
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
    use super::{expanded_header, startup_header};

    fn paths(names: &[&str]) -> Vec<std::path::PathBuf> {
        names.iter().map(std::path::PathBuf::from).collect()
    }

    // Verifies: gh #131 - the header lists AGENTS.md paths when present,
    // with the resource counts behind them.
    #[test]
    // **Changed by gh #400 gap 2's brief**: the default launch is now one compact
    // line, so this row reads the values through `startup_header_with` with the
    // header expanded - the same facts, the new default. The assertions below are
    // unchanged; only which function produces the lines moved.
    fn the_header_lists_context_paths_and_counts() {
        let lines = expanded_header(
            "0.6.0",
            "0.6",
            &paths(&["/home/u/.lca/AGENTS.md", "/w/.lca/AGENTS.md"]),
            3,
            2,
            1,
            lca_ui::CollisionSummary::default(),
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
