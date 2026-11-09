//! Opening the session and store, and the two registry adapters the interface
//! pulls through (FR-UI-1/FR-UI-6).
//!
//! Split from `tui/mod.rs` at gate 11's 1,200-line ceiling (gh #543). This is
//! the *before-the-interface* half of the composition root: `open` resolves the
//! store, the grant store, the merged config and the session to show, `initial_view`
//! frames a resumed transcript, `resolve_model_id` picks the model when the config
//! names none, and the two `region_*` functions adapt an extension registry into
//! the interface's render/interact seams. `Ui::new` calls them in that order.
//!
//! Nothing here holds a `Ui`, so the file is a plain module of functions - the
//! seam `mod.rs` already had, since `Ui` does not exist yet when these run.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lca_config::Config;
use lca_core::ExtensionRegistry;
use lca_permissions::GrantStore;
use lca_provider::Provider;
use lca_session::{Session, SessionStore};
use lca_ui::{RegionInteractor, RegionRenderer};

use crate::lock;

/// The session-and-store setup, before the extension registry exists.
pub(super) struct Opened {
    // `pub(super)` == `pub(in crate::tui)`: the identical reach these fields had
    // as private fields of `tui/mod.rs` (readable by `Ui::new`, nowhere else).
    pub(super) data: PathBuf,
    /// Held for a volatile run: dropping it removes the temp store.
    pub(super) _volatile: Option<crate::invoke::VolatileStore>,
    pub(super) store: Arc<SessionStore>,
    pub(super) config: Config,
    pub(super) grants: Arc<Mutex<GrantStore>>,
    pub(super) trusted: bool,
    pub(super) current_session: Arc<Mutex<Session>>,
    pub(super) session_id: String,
    pub(super) provider_name: String,
    pub(super) initial_head: Vec<String>,
    pub(super) initial_records: Vec<lca_protocol::Record>,
    pub(super) initial_tail: Vec<String>,
    pub(super) update_notice: Arc<std::sync::OnceLock<String>>,
}

/// Open the store, the grant store, the merged configuration, and the
/// session to show (resumed or fresh).
pub(super) fn open(
    cwd: &Path,
    resume: Option<&str>,
    fork: Option<&str>,
    session_id: Option<&str>,
    yolo: bool,
    allow_host: &[String],
    flags: &crate::CliFlags,
) -> anyhow::Result<Opened> {
    let data = crate::data_dir();
    // gh #69: a volatile run stores sessions in a temp dir dropped with
    // the interface; grants stay on the real store (trust is not
    // session state).
    let volatile = flags
        .no_session
        .then(crate::invoke::VolatileStore::create)
        .transpose()
        .map_err(|err| anyhow::anyhow!("cannot create a volatile session store: {err}"))?;
    let session_root = volatile
        .as_ref()
        .map(|volatile| volatile.root().to_path_buf())
        .unwrap_or_else(|| data.clone());
    let store = Arc::new(SessionStore::new(session_root.clone()));
    let grants = Arc::new(Mutex::new(
        GrantStore::open(&data.join("grants.json"))
            .map_err(|err| anyhow::anyhow!("cannot open the grant store: {err}"))?,
    ));
    // gh #80: CLI trust overrides land before the config merge, so the
    // project-file gate sees them.
    let trust_default = crate::trust_default_fallback();
    crate::apply_startup_trust(&mut lock(&grants), cwd, flags, &trust_default);
    let trusted = lock(&grants).is_trusted(cwd);
    let config = crate::load_config_flags(cwd, &lock(&grants), false, yolo, flags)?;
    // Today's update check, if enabled and due: stamped, then spawned - the
    // startup path never waits on it (FR-CFG-6), and the status line picks
    // the finding up from the shared cell once it lands.
    let update_notice = Arc::new(std::sync::OnceLock::new());
    // gh #71: `--offline` silences automatic network activity; the
    // model request itself still goes out (that is the run).
    if !flags.offline {
        crate::update::spawn(config.update_check(false), Some(update_notice.clone()));
    }
    let session =
        crate::resolve_session(&store, cwd, resume, fork, session_id, flags.name.as_deref())
            .map_err(|err| anyhow::anyhow!("{err}"))?;
    let current_session = Arc::new(Mutex::new(session.clone()));
    let provider_name = config.provider().to_string();
    let (mut initial_head, initial_records) = initial_view(&store, &session);
    // gh #122: a fresh session replays no session-start (nothing
    // written yet), but the orientation header still shows - from
    // the working directory, not the log.
    if initial_records.is_empty() {
        initial_head.push(format!(
            "[session in {}]",
            lca_ui::display_path(&cwd.display().to_string())
        ));
    }
    // ADR-0042: the mode applies to the shared grant store, so the model's
    // tool calls and an extension's `process` calls answer alike. The banner
    // leads the transcript: hands-free must never mean invisible.
    if let Some(banner) = crate::apply_permission_mode(&config, &mut lock(&grants)) {
        initial_head.insert(0, banner.to_string());
    }
    // `--allow-host`: a one-run grant, attached to the session set before
    // any turn runs and recorded once (gh #29, QA-004).
    crate::net_consent::attach_allow_hosts(&grants, cwd, allow_host, &store, &session)
        .map_err(|err| anyhow::anyhow!("{err}"))?;
    Ok(Opened {
        _volatile: volatile,
        data,
        store,
        config,
        grants,
        trusted,
        current_session,
        session_id: session.id().to_string(),
        provider_name,
        initial_head,
        initial_records,
        initial_tail: Vec::new(),
        update_notice,
    })
}

/// The initial transcript lines: the records, the truncation/skip
/// warnings, and the ungranted-endpoint note (FR-PERM-16).
/// The resumed transcript's framing: head warnings (top of screen) and the
/// session's records, which the interface replays with the live rendering
/// (FR-UI-7) instead of the flattened lines this used to return.
pub(super) fn initial_view(
    store: &SessionStore,
    session: &Session,
) -> (Vec<String>, Vec<lca_protocol::Record>) {
    let read = match store.read_with(session, lca_session::ViewMode::Display) {
        Ok(read) => read,
        Err(err) => {
            return (
                vec![format!("error: cannot read the session: {err}")],
                Vec::new(),
            );
        }
    };
    let mut head: Vec<String> = Vec::new();
    // A session that loaded with a truncation or a skipped line must say so:
    // a short or empty transcript with no explanation reads as data loss.
    if read.truncated {
        head.push(
            "warning: the session log was truncated; only the records that loaded are shown"
                .to_string(),
        );
    } else if read.skipped_unknown > 0 {
        head.push(format!(
            "warning: {} record(s) were skipped (unknown type or version)",
            read.skipped_unknown
        ));
    }
    (head, read.records)
}

/// The configured model, or the first model *in the enabled scope* when the
/// provider is ready and the configuration names none: a narrower scope must not
/// start the session on a model its own cycle cannot reach (gh #8).
pub(super) fn resolve_model_id(
    config: &Config,
    provider_is_ready: bool,
    provider: &dyn Provider,
) -> String {
    let configured = config.model().unwrap_or_default();
    if !configured.is_empty() {
        configured.to_string()
    } else if provider_is_ready {
        // An empty id counts as no model: a provider that answered a model
        // probe with something unparseable must not leave the session with
        // a blank model label and a `complete` call the extension refuses.
        crate::models::filter_enabled(provider.list_models(), config.models_enabled())
            .into_iter()
            .map(|model| model.id)
            .find(|id| !id.is_empty())
            .unwrap_or_default()
    } else {
        String::new()
    }
}

/// The extension render-region view (FR-UI-1's pull table).
pub(super) fn region_renderer(registry: Arc<ExtensionRegistry>) -> Option<RegionRenderer> {
    Some(std::sync::Arc::new(
        move |region: &str| -> Vec<(String, lca_protocol::WidgetTree)> {
            registry
                .enabled()
                .filter(|handle| handle.ui_regions().iter().any(|r| r == region))
                .filter_map(|handle| {
                    handle
                        .render(region)
                        .ok()
                        .flatten()
                        .map(|tree| (handle.name().to_string(), tree))
                })
                .collect()
        },
    ) as RegionRenderer)
}

/// The extension ui-event router (FR-UI-6's only source).
pub(super) fn region_interactor(registry: Arc<ExtensionRegistry>) -> Option<RegionInteractor> {
    Some(std::sync::Arc::new(
        move |region: &str,
              input: &lca_protocol::UiInput|
              -> Option<(String, lca_protocol::UiEffect)> {
            registry
                .enabled()
                .filter(|handle| handle.ui_regions().iter().any(|r| r == region))
                .find_map(|handle| {
                    handle
                        .on_ui_event(region, input)
                        .ok()
                        .map(|effect| (handle.name().to_string(), effect))
                })
        },
    ) as RegionInteractor)
}
