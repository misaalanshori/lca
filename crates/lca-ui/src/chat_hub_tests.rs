//! The `/extensions` hub journey: open, filter, navigate, consent,
//! approve, deny, close. Uses the `chat_tests` fixtures like the other
//! overlay journeys.

use super::{chat, options};
use crate::chat_hub::{HubData, InstallConsent};
use std::sync::Arc;

fn hub_chat() -> (
    crate::chat::Chat,
    Arc<std::sync::Mutex<Vec<InstallConsent>>>,
) {
    let mut opts = options();
    let confirmed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let confirm_host = confirmed.clone();
    opts.hooks.fetch_extensions = Some(Arc::new(move || {
        let rows = crate::chat_hub::fixture_rows();
        let installed = rows
            .iter()
            .filter(|row| row.installed)
            .cloned()
            .collect::<Vec<_>>();
        let available = rows
            .into_iter()
            .filter(|row| !row.installed)
            .collect::<Vec<_>>();
        HubData {
            installed,
            available,
            notice: None,
        }
    }));
    opts.hooks.confirm_install = Some(Arc::new(move |consent| {
        confirm_host
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(consent);
    }));
    opts.hooks.poll_install = Some(Arc::new(|| None));
    let mut chat = chat();
    chat.world.options.hooks = opts.hooks;
    (chat, confirmed)
}

// Verifies: #227 - opening `/extensions` shows installed rows grouped
// ahead of catalog rows with provenance badges on each.
#[test]
fn hub_opens_with_sections_and_provenance() {
    let (mut chat, _) = hub_chat();
    chat.dispatch_extensions();
    assert!(chat.extension_hub.is_some(), "the hub opens");
    let hub = chat.extension_hub.as_mut().expect("hub");
    hub.drain_hub();
    assert!(!hub.loading, "rows settle the loader");
    assert_eq!(hub.matches.len(), 3);
    let first = hub.selected_row().expect("selection");
    assert!(first.installed, "installed rows lead");
}

// Verifies: #227 - typing filters to matching rows; Enter on an
// available row opens consent naming publisher, origin, artifact,
// and capabilities; approving fires the host install.
#[test]
fn hub_filter_enter_consent_and_approve() {
    let (mut chat, confirmed) = hub_chat();
    chat.dispatch_extensions();
    chat.extension_hub.as_mut().expect("hub").drain_hub();
    // Filter to the custom row.
    for c in "acme".chars() {
        chat.handle_extension_hub_key(&c.to_string(), None);
    }
    {
        let hub = chat.extension_hub.as_ref().expect("hub");
        assert_eq!(hub.matches.len(), 1);
        assert_eq!(
            hub.selected_row().map(|row| row.name.as_str()),
            Some("acme-deploy")
        );
    }
    // Enter opens consent with the row's provenance and capabilities.
    chat.handle_extension_hub_key("", Some("enter"));
    let consent = chat.install_consent.clone().expect("consent opens");
    assert_eq!(consent.name, "acme-deploy");
    assert!(consent.publisher.contains("Third-Party"));
    assert_eq!(consent.artifact_ref, "ghcr.io/acme/deploy:1.2.0");
    assert_eq!(consent.capabilities, vec!["process"]);
    // Approve fires the host install with the same data.
    chat.handle_install_consent_key("", Some("enter"));
    assert!(chat.install_consent.is_none(), "consent settles");
    let confirmed = confirmed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0], consent);
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap_or("")
            .contains("acme-deploy"),
        "progress shows: {:?}",
        chat.world.notice
    );
}

// Verifies: #227 - denying (or Escaping) consent returns to the hub
// with nothing fired and nothing written.
#[test]
fn hub_deny_returns_without_firing() {
    let (mut chat, confirmed) = hub_chat();
    chat.dispatch_extensions();
    chat.extension_hub.as_mut().expect("hub").drain_hub();
    // Down to the first available row, then Enter opens consent.
    chat.handle_extension_hub_key("", Some("down"));
    chat.handle_extension_hub_key("", Some("enter"));
    assert!(chat.install_consent.is_some());
    chat.handle_install_consent_key("", Some("escape"));
    assert!(chat.install_consent.is_none(), "consent dismisses");
    assert!(chat.extension_hub.is_some(), "the hub stays open");
    assert!(
        confirmed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty(),
        "no install fires on deny"
    );
    // Esc closes the hub itself.
    chat.handle_extension_hub_key("", Some("escape"));
    assert!(chat.extension_hub.is_none(), "esc closes");
}

// Verifies: #227 - Enter on an installed row reports instead of
// installing: installs only flow for available rows.
#[test]
fn hub_enter_on_installed_reports() {
    let (mut chat, confirmed) = hub_chat();
    chat.dispatch_extensions();
    chat.extension_hub.as_mut().expect("hub").drain_hub();
    chat.handle_extension_hub_key("", Some("enter"));
    assert!(chat.install_consent.is_none(), "no consent for installed");
    assert!(
        confirmed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
    );
    assert!(
        chat.world
            .notice
            .as_deref()
            .unwrap_or("")
            .contains("already installed"),
        "the notice says so: {:?}",
        chat.world.notice
    );
}
