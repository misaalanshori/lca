//! `Chat` dialog rows (gh #124, gh #172), split from `chat_tests.rs`
//! for the workspace's 1,200-line file ceiling. The fixtures
//! (`options`, `chat`) stay in `chat_tests.rs` and are imported from it.

use super::tests::chat;
use crate::state::Action;

// Verifies: gh #124 - a confirm dialog answers on y/n and closes, and
// the worker hears it through the exchange channel.
#[test]
fn a_confirm_dialog_answers_and_closes() {
    use lca_protocol::{DialogAnswer, UiDialog};
    let mut chat = chat();
    let (respond, response) = std::sync::mpsc::sync_channel(1);
    chat.world.dialog = Some(crate::state::DialogModal {
        exchange: crate::state::DialogExchange {
            dialog: UiDialog::Confirm {
                title: "t".into(),
                message: "m".into(),
            },
            respond,
        },
        query: String::new(),
        matches: Vec::new(),
        selected: 0,
        input: String::new(),
    });
    chat.handle_key("n");
    assert_eq!(response.try_recv().ok(), Some(DialogAnswer::Confirm(false)));
    assert!(chat.world.dialog.is_none(), "answered dialogs close");
}

// Verifies: gh #124 - a select filters on typing and picks the
// highlight on enter; escape dismisses to None.
#[test]
fn a_select_dialog_filters_and_picks() {
    use lca_protocol::{DialogAnswer, UiDialog};
    let mut chat = chat();
    let (respond, response) = std::sync::mpsc::sync_channel(1);
    let options = vec!["alpha".to_string(), "beta".to_string()];
    chat.world.dialog = Some(crate::state::DialogModal {
        exchange: crate::state::DialogExchange {
            dialog: UiDialog::Select {
                title: "t".into(),
                options: options.clone(),
            },
            respond,
        },
        query: String::new(),
        matches: vec![0, 1],
        selected: 0,
        input: String::new(),
    });
    chat.handle_key("b");
    assert_eq!(chat.world.dialog.as_ref().unwrap().matches, vec![1]);
    chat.handle_key("\r");
    assert_eq!(
        response.try_recv().ok(),
        Some(DialogAnswer::Select(Some("beta".to_string())))
    );
    assert!(chat.world.dialog.is_none());
}

// Verifies: gh #124 - an input dialog edits one line; enter submits,
// escape dismisses, and an empty submit is None (the WIT contract).
#[test]
fn an_input_dialog_edits_and_submits() {
    use lca_protocol::{DialogAnswer, UiDialog};
    let mut chat = chat();
    let (respond, response) = std::sync::mpsc::sync_channel(1);
    chat.world.dialog = Some(crate::state::DialogModal {
        exchange: crate::state::DialogExchange {
            dialog: UiDialog::Input {
                label: "l".into(),
                placeholder: None,
            },
            respond,
        },
        query: String::new(),
        matches: Vec::new(),
        selected: 0,
        input: String::new(),
    });
    for c in "hi".chars() {
        chat.handle_key(&c.to_string());
    }
    chat.handle_key("\r");
    assert_eq!(
        response.try_recv().ok(),
        Some(DialogAnswer::Input(Some("hi".to_string())))
    );

    let (respond, response) = std::sync::mpsc::sync_channel(1);
    chat.world.dialog = Some(crate::state::DialogModal {
        exchange: crate::state::DialogExchange {
            dialog: UiDialog::Input {
                label: "l".into(),
                placeholder: None,
            },
            respond,
        },
        query: String::new(),
        matches: Vec::new(),
        selected: 0,
        input: String::new(),
    });
    chat.handle_key("\x1b");
    assert_eq!(response.try_recv().ok(), Some(DialogAnswer::Input(None)));
}

// Verifies: gh #172 - the panel toggle closes an open panel (the open
// panel otherwise eats the toggle key itself, trapping `/exit`).
#[test]
fn the_panel_toggle_closes_an_open_panel() {
    let mut chat = chat();
    chat.world.panel_open = true;
    // `alt+x` is the `app.panel.toggle` default.
    assert_eq!(chat.handle_key("\x1bx"), Action::Continue);
    assert!(!chat.world.panel_open);
    assert_eq!(chat.handle_key("\x1bx"), Action::Continue);
    assert!(chat.world.panel_open);
}
