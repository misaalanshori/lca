//! The `/ext` extension manager journey (gh #3): list installed
//! extensions with states, toggle enable/disable, pick the compaction
//! strategy. Uses the `chat_tests` fixtures like the other journeys.

use super::chat;
use crate::chat_extman::{ManagedData, ManagedRow};
use std::sync::{Arc, Mutex};

fn managed_chat() -> (
    crate::chat::Chat,
    Arc<Mutex<Vec<(String, bool)>>>,
    Arc<Mutex<Vec<Option<String>>>>,
) {
    let mut chat = chat();
    let toggles = Arc::new(Mutex::new(Vec::new()));
    let strategies = Arc::new(Mutex::new(Vec::new()));
    let rows = vec![
        ManagedRow::Extension {
            name: "alpha".to_string(),
            enabled: true,
            compaction: false,
        },
        ManagedRow::Extension {
            name: "compacter".to_string(),
            enabled: true,
            compaction: true,
        },
    ];
    chat.world.options.hooks.list_managed = Some(Arc::new(move || ManagedData {
        extensions: rows.clone(),
        strategy: None,
    }));
    let seen_toggles = toggles.clone();
    chat.world.options.hooks.toggle_managed = Some(Arc::new(move |name, enabled| {
        seen_toggles.lock().expect("toggles").push((name.clone(), enabled));
        format!("{} {}", name, if enabled { "enabled" } else { "disabled" })
    }));
    let seen_strategies = strategies.clone();
    chat.world.options.hooks.set_managed_strategy = Some(Arc::new(move |name| {
        seen_strategies.lock().expect("strategies").push(name.clone());
        "strategy set".to_string()
    }));
    (chat, toggles, strategies)
}

// Verifies: gh #3 - `/ext` lists installed extensions with their
// states; Enter on a row toggles it through the host hook and the
// row updates.
#[test]
fn toggle_calls_host_and_updates_the_row() {
    let (mut chat, toggles, _) = managed_chat();
    chat.open_extension_manager();
    assert!(chat.ext_manager_open());
    // First row is alpha (enabled): Enter disables it.
    chat.handle_extension_manager_key("", Some("enter"));
    assert_eq!(
        *toggles.lock().expect("toggles"),
        vec![("alpha".to_string(), false)]
    );
    assert!(chat.world.notice.as_deref().unwrap_or("").contains("alpha"));
}

// Verifies: gh #3 - the strategy section picks the compaction
// strategy through the host hook; Automatic clears the preference.
#[test]
fn strategy_pick_calls_hook() {
    let (mut chat, _, strategies) = managed_chat();
    chat.open_extension_manager();
    // Rows lead with the strategy section: Automatic, then the
    // compaction extensions. One step down reaches compacter.
    chat.handle_extension_manager_key("", Some("down"));
    chat.handle_extension_manager_key("", Some("enter"));
    assert_eq!(
        *strategies.lock().expect("strategies"),
        vec![Some("compacter".to_string())]
    );
}
