//! The `/extensions` hub (#227): installed extensions plus catalog
//! rows with provenance, filterable, installing through consent.
//! The host supplies rows and runs installs; this file owns the
//! picker state, the consent step, and their tests.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use super::chat::Chat;
use super::chat_pickers::HubRow;
use crate::state::Action;

/// One hub fetch: installed rows plus catalog rows, assembled
/// host-side (installed tree + fetched catalogs, merged).
#[derive(Debug, Clone, Default)]
pub struct HubData {
    /// Installed rows (the INSTALLED section).
    pub installed: Vec<HubRow>,
    /// Catalog rows (grouped by source downstream).
    pub available: Vec<HubRow>,
    /// Offline/degraded guidance: shown as a notice when present.
    pub notice: Option<String>,
}

/// The host's synchronous hub fetch: installed plus cached catalog
/// rows, merged. Slow first fetches run on the chat-spawned worker
/// (never the interface thread).
pub type FetchExtensions = Arc<dyn Fn() -> HubData + Send + Sync>;

/// What the install-consent step asks: everything the modal shows,
/// all of it from the row (nothing is fetched before consent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallConsent {
    /// Extension name.
    pub name: String,
    /// Release version.
    pub version: String,
    /// Publisher line: the source name carries the provenance itself
    /// (`Official (misaalanshori/lca)` vs a vendor name), with one
    /// bracketed word — `[official]` or `[third-party]`, the same
    /// vocabulary as the hub list header — so one source has one label.
    pub publisher: String,
    /// The exact catalog document the row came from.
    pub source_url: String,
    /// The exact artifact ref being downloaded.
    pub artifact_ref: String,
    /// Declared capability names.
    pub capabilities: Vec<String>,
}

/// The host's install confirmation: spawn the install worker now that
/// consent exists, and report back through [`InstallPoll`].
pub type ConfirmInstall = Arc<dyn Fn(InstallConsent) + Send + Sync>;

/// A finished background install, posted once as a notice.
pub type InstallPoll = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// The open hub: sections render from one flat match list, headers
/// emitted on section change, so selection never lands on a header.
pub struct ExtensionHub {
    /// Every row, installed first, then catalog order.
    pub rows: Vec<HubRow>,
    /// The typed filter query.
    pub query: String,
    /// Indices into `rows` that match.
    pub matches: Vec<usize>,
    /// The highlighted row (an index into `matches`).
    pub selected: usize,
    /// A background fetch is still running: the body shows a loading
    /// line until the rows land.
    pub loading: bool,
    /// The spinner frame's clock.
    pub loading_advanced: std::time::Instant,
    /// The visible spinner frame.
    pub loading_frame: usize,
    /// The background fetch's rows, collected by `poll_hub`.
    pub refresh: Option<Receiver<HubData>>,
}

impl ExtensionHub {
    /// Open over installed rows with catalog rows loading behind.
    pub fn loading(installed: Vec<HubRow>) -> ExtensionHub {
        let mut hub = ExtensionHub {
            rows: installed,
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            loading: true,
            loading_advanced: std::time::Instant::now(),
            loading_frame: 0,
            refresh: None,
        };
        hub.refilter();
        hub
    }

    /// Open over a complete row set (tests, cached-instant hosts).
    pub fn ready(rows: Vec<HubRow>) -> ExtensionHub {
        let mut hub = ExtensionHub {
            rows,
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            loading: false,
            loading_advanced: std::time::Instant::now(),
            loading_frame: 0,
            refresh: None,
        };
        hub.refilter();
        hub
    }

    /// Advance the loading spinner. Returns `true` when the frame
    /// moved, so the loop knows to repaint.
    pub fn tick(&mut self) -> bool {
        if !self.loading || self.loading_advanced.elapsed() < crate::separator::FRAME_MS {
            return false;
        }
        self.loading_frame += 1;
        self.loading_advanced = std::time::Instant::now();
        true
    }

    /// Re-filter after a query change, keeping the selection in range.
    /// Matches name and description; headers are not rows, so every
    /// match stays selectable.
    pub fn refilter(&mut self) {
        let needle = self.query.to_lowercase();
        self.matches = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                row.name.to_lowercase().contains(&needle)
                    || row.description.to_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect();
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    /// Fill landed rows (installed first, then catalog order) and
    /// settle the loader.
    pub fn apply_rows(&mut self, data: HubData) {
        let mut rows = data.installed;
        rows.extend(data.available);
        self.rows = rows;
        self.loading = false;
        self.refilter();
    }

    /// The highlighted row, if any.
    pub fn selected_row(&self) -> Option<&HubRow> {
        self.matches
            .get(self.selected)
            .and_then(|index| self.rows.get(*index))
    }

    /// Collect a landed background fetch. Returns `true` when rows
    /// landed (the loop repaints).
    pub(crate) fn poll_hub(&mut self) -> bool {
        let data = match self.refresh.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(data)) => data,
            _ => return false,
        };
        self.refresh = None;
        self.apply_rows(data);
        true
    }

    /// The blocking drain tests use to make the background arrival
    /// deterministic.
    #[cfg(test)]
    pub(crate) fn drain_hub(&mut self) {
        if let Some(rx) = self.refresh.take()
            && let Ok(data) = rx.recv()
        {
            self.apply_rows(data);
        }
    }
}

impl Chat {
    /// Open the hub over a loader; installed plus catalog rows land
    /// through `poll_hub` (the `/model` loader shape: never block
    /// the interface thread on a fetch).
    pub(crate) fn open_extension_hub(&mut self) {
        let Some(fetch) = self.world.options.hooks.fetch_extensions.clone() else {
            self.world.notice = Some("extensions are not available in this host".to_string());
            return;
        };
        let mut hub = ExtensionHub::loading(Vec::new());
        let (tx, rx) = std::sync::mpsc::channel();
        hub.refresh = Some(rx);
        self.extension_hub = Some(hub);
        std::thread::spawn(move || {
            let _ = tx.send(fetch());
        });
    }

    /// The `/extensions` entry.
    pub(crate) fn dispatch_extensions(&mut self) -> Action {
        self.open_extension_hub();
        Action::Continue
    }

    /// Handle a key while the hub owns the keyboard: navigate, filter,
    /// install through consent, close.
    pub(crate) fn handle_extension_hub_key(
        &mut self,
        data: &str,
        key: Option<&str>,
    ) -> Option<Action> {
        let mut hub = self.extension_hub.take()?;
        match key {
            Some("escape") => None,
            Some("up") => {
                hub.selected = hub.selected.saturating_sub(1);
                self.extension_hub = Some(hub);
                None
            }
            Some("down") => {
                if !hub.matches.is_empty() {
                    hub.selected = (hub.selected + 1).min(hub.matches.len() - 1);
                }
                self.extension_hub = Some(hub);
                None
            }
            Some("enter") => {
                let next = hub.selected_row().cloned();
                self.extension_hub = Some(hub);
                match next {
                    Some(row) if row.installed => {
                        self.world.notice =
                            Some(format!("{} {} is already installed", row.name, row.version));
                    }
                    Some(row) => {
                        self.install_consent = Some(hub_install_consent(&row));
                    }
                    None => {}
                }
                None
            }
            Some("backspace") => {
                hub.query.pop();
                hub.refilter();
                self.extension_hub = Some(hub);
                None
            }
            _ => {
                // Printable keys filter; anything else stays put.
                if let Some(text) = data.chars().find(|c| !c.is_control()) {
                    hub.query.push(text);
                    hub.refilter();
                    self.extension_hub = Some(hub);
                } else {
                    self.extension_hub = Some(hub);
                }
                None
            }
        }
    }

    /// Answer the install-consent modal: approve fires the host's
    /// install worker (the result arrives through `poll_install`),
    /// deny returns to the hub.
    pub(super) fn answer_install_consent(&mut self, approved: bool) {
        let Some(consent) = self.install_consent.take() else {
            return;
        };
        if !approved {
            return;
        }
        let Some(confirm) = self.world.options.hooks.confirm_install.clone() else {
            self.world.notice = Some("installing is not available in this host".to_string());
            return;
        };
        self.world.notice = Some(format!("installing {}…", consent.name));
        confirm(consent);
    }

    /// Keys while the install-consent modal owns the keyboard:
    /// `y`/Enter approves, `n`/Escape returns to the hub; anything
    /// else is swallowed so no input lands under the modal.
    pub(crate) fn handle_install_consent_key(
        &mut self,
        data: &str,
        key: Option<&str>,
    ) -> Option<Action> {
        self.install_consent.as_ref()?;
        match key {
            Some("escape") | Some("n") => {
                self.answer_install_consent(false);
            }
            Some("enter") => {
                self.answer_install_consent(true);
            }
            _ => {
                if data == "y" {
                    self.answer_install_consent(true);
                }
            }
        }
        Some(Action::Continue)
    }

    /// Collect a finished background install. Returns `true` when a
    /// notice landed (the loop repaints).
    pub(crate) fn poll_install(&mut self) -> bool {
        let Some(poll) = self.world.options.hooks.poll_install.clone() else {
            return false;
        };
        match poll() {
            Some(notice) => {
                self.world.notice = Some(notice);
                true
            }
            None => false,
        }
    }
}

/// The consent modal's data for one hub row: publisher, exact origin,
/// artifact ref, and declared capabilities - the FR-PERM-2 surface.
pub fn hub_install_consent(row: &HubRow) -> InstallConsent {
    InstallConsent {
        name: row.name.clone(),
        version: row.version.clone(),
        publisher: if row.official {
            format!("{} [official]", row.source_name)
        } else {
            format!("{} [third-party]", row.source_name)
        },
        source_url: row.source_url.clone(),
        artifact_ref: row.oci_ref.clone(),
        capabilities: row.capabilities.clone(),
    }
}

/// Rows shared by the hub's unit tests: one installed, one official,
/// one custom.
#[cfg(test)]
pub fn fixture_rows() -> Vec<HubRow> {
    vec![
        HubRow {
            name: "openai-compatible".to_string(),
            version: "1.0.0".to_string(),
            capabilities: vec!["net".to_string()],
            description: "Compatible endpoints.".to_string(),
            oci_ref: String::new(),
            source_name: String::new(),
            source_url: String::new(),
            official: false,
            installed: true,
        },
        HubRow {
            name: "openrouter".to_string(),
            version: "0.1.0".to_string(),
            capabilities: vec!["net".to_string(), "oauth".to_string()],
            description: "Many models.".to_string(),
            oci_ref: "ghcr.io/misaalanshori/lca/openrouter:abi-0.6".to_string(),
            source_name: "Official (misaalanshori/lca)".to_string(),
            source_url: "https://example.invalid/official.toml".to_string(),
            official: true,
            installed: false,
        },
        HubRow {
            name: "acme-deploy".to_string(),
            version: "1.2.0".to_string(),
            capabilities: vec!["process".to_string()],
            description: "Team deploys.".to_string(),
            oci_ref: "ghcr.io/acme/deploy:1.2.0".to_string(),
            source_name: "Acme Corp".to_string(),
            source_url: "https://example.invalid/acme.toml".to_string(),
            official: false,
            installed: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub() -> ExtensionHub {
        ExtensionHub::ready(fixture_rows())
    }

    // Verifies: #227 - sections group installed first, then catalog
    // order, and filtering never strands the selection.
    #[test]
    fn hub_groups_filter_and_keeps_selection() {
        let mut hub = hub();
        assert_eq!(hub.matches.len(), 3);
        hub.query = "open".to_string();
        hub.refilter();
        assert_eq!(hub.matches.len(), 2);
        hub.selected = 99;
        hub.refilter();
        assert_eq!(hub.selected, 1);
        hub.query = "zzz".to_string();
        hub.refilter();
        assert!(hub.matches.is_empty());
        assert_eq!(hub.selected, 0);
    }

    // Verifies: #227 - the loader settles: background rows land
    // through the drain and clear the loading state.
    #[test]
    fn loader_settles_when_rows_land() {
        let mut hub = ExtensionHub::loading(Vec::new());
        assert!(hub.loading);
        let (tx, rx) = std::sync::mpsc::channel();
        hub.refresh = Some(rx);
        tx.send(HubData {
            installed: Vec::new(),
            available: fixture_rows(),
            notice: None,
        })
        .expect("send");
        hub.drain_hub();
        assert!(!hub.loading);
        assert_eq!(hub.matches.len(), 3);
        assert!(hub.refresh.is_none());
    }

    // Verifies: #227 - the consent carries publisher, exact origin,
    // artifact ref, and declared capabilities, badged by provenance.
    #[test]
    fn install_consent_badges_provenance() {
        let rows = fixture_rows();
        let official = hub_install_consent(&rows[1]);
        assert!(official.publisher.contains("First-Party Verified"));
        assert_eq!(
            official.artifact_ref,
            "ghcr.io/misaalanshori/lca/openrouter:abi-0.6"
        );
        assert_eq!(official.capabilities, vec!["net", "oauth"]);
        assert_eq!(official.source_url, "https://example.invalid/official.toml");
        let custom = hub_install_consent(&rows[2]);
        assert!(custom.publisher.contains("Third-Party"));
    }
}
