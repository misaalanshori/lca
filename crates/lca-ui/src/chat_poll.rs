//! Per-tick polls: background steps report back here so slow work
//! never blocks the loop. Split from `run.rs` for the workspace file
//! ceiling (gate 11). Behaviour unchanged.

use super::chat::Chat;
use crate::ModelPicker;

pub(crate) fn poll_login(chat: &mut Chat) -> bool {
    let Some(poll) = chat.world.options.hooks.poll_login.clone() else {
        return false;
    };
    match poll() {
        Some(next) => {
            chat.apply_login_next(next);
            true
        }
        None => false,
    }
}

/// Open the `/model` picker when the host has rows ready for it - the
/// consent flow's second step (gh #31 review): the ask ran off-thread, so
/// the modal could render, and the list it produced arrives here. One-shot:
/// the host clears its cell as it hands the rows over.
pub(crate) fn poll_pending_models(chat: &mut Chat) -> bool {
    let Some(pending) = chat.world.options.pending_models.clone() else {
        return false;
    };
    let rows = pending();
    if rows.is_empty() {
        return false;
    }
    // gh #232: consent rows replace a loading picker (the background
    // discovery it waited on is over); any other open picker keeps
    // the floor.
    let loading = chat.model_picker.as_ref().is_some_and(|open| open.loading);
    if chat.model_picker.is_some() && !loading {
        return false;
    }
    // gh #233: consent grants landed with these rows; the snapshot
    // predates them.
    chat.invalidate_models();
    chat.model_picker = Some(ModelPicker::new(rows));
    true
}

/// anything changed). Repaints while it runs so it ticks visibly. It needs
/// no turn state, so it is not a `TurnState` method.
pub(crate) fn tick_permission(chat: &mut Chat) -> bool {
    if chat
        .world
        .permission
        .as_ref()
        .is_some_and(|m| m.deadline.is_some_and(|d| std::time::Instant::now() >= d))
    {
        if let Some(modal) = chat.world.permission.take()
            && let Some(respond) = modal.respond
        {
            let _ = respond.send(lca_permissions::Decision::Once);
        }
        true
    } else {
        chat.world.permission.is_some()
    }
}
