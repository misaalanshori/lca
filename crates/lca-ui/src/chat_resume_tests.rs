//! `/resume` picker rows (gh #75): search, labels, and delete.
//!
//! Split from `chat_overlay_tests.rs` under the workspace's 1,200-line
//! file ceiling. Fixtures (`options`, `chat`) stay in `chat_tests.rs`.

use super::*;
use super::options;
use std::sync::Arc;

// Verifies: gh #75 - Ctrl+D in `/resume` asks on the row, `y`
// trashes through the host hook and rebuilds the list.
#[test]
fn resume_delete_confirms_and_trashes() {
    use std::sync::Mutex;
    let trashed = Arc::new(Mutex::new(Vec::new()));
    let written = trashed.clone();
    let mut opts = options();
    let entries = Arc::new(Mutex::new(vec![
        ("a".to_string(), "alpha".to_string()),
        ("b".to_string(), "beta".to_string()),
    ]));
    let listed = entries.clone();
    opts.hooks.session_list = Some(Arc::new(move || {
        listed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(id, title)| crate::resume::SessionEntry {
                id: id.clone(),
                title: title.clone(),
                messages: 1,
                age: "now".to_string(),
                labels: Vec::new(),
            })
            .collect()
    }));
    opts.hooks.delete_session = Some(Arc::new(move |id: &str| {
        written
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(id.to_string());
        entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|(kept, _)| kept != id);
        Ok(format!("trashed '{id}'"))
    }));
    opts.hooks.current_session_id = Some(Arc::new(|| "b".to_string()));
    let mut chat = Chat::new(opts, Arc::new(KeybindingsManager::new()));
    for c in "/resume".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert!(chat.resume_picker.is_some(), "the picker opens");
    chat.handle_key("\x1b[B"); // to beta
    chat.handle_key("\x04"); // Ctrl+D on the live session refuses
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("that session is open here - switch away first"),
        "the live session refuses"
    );
    assert!(chat.resume_picker.is_some(), "the picker stays open");
    chat.handle_key("\x1b[A"); // back to alpha
    chat.handle_key("\x04");
    let picker = chat.resume_picker.as_ref().expect("open");
    assert_eq!(picker.confirming, Some(0), "the row asks");
    chat.handle_key("n");
    assert!(
        chat.resume_picker
            .as_ref()
            .expect("open")
            .confirming
            .is_none(),
        "n cancels"
    );
    assert!(trashed.lock().unwrap().is_empty(), "nothing trashed");
    chat.handle_key("\x04");
    chat.handle_key("y");
    assert_eq!(
        trashed.lock().unwrap().as_slice(),
        &["a".to_string()],
        "y trashes by id"
    );
    assert_eq!(
        chat.world.notice.as_deref(),
        Some("trashed 'a'"),
        "the hook's notice shows"
    );
    let picker = chat.resume_picker.as_ref().expect("open");
    assert_eq!(picker.entries.len(), 1, "the list rebuilds");
    assert_eq!(picker.entries[0].id, "b");
}
