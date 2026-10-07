//! Host-rendered dialog drain (gh #124): one question at a time from
//! the session's dialog channel, split from `run.rs` for the workspace's
//! 1,200-line file ceiling. Behavior unchanged.

use std::sync::mpsc::Receiver;

use crate::chat::Chat;
use crate::chat_commands::{paste_text, printable};
use crate::state::{Action, DialogExchange, DialogModal};

/// Drain one dialog exchange into the chat (true when anything changed):
/// a notification notices and answers without chrome, anything else opens
/// the modal - only when no permission modal and no dialog is already open.
pub fn drain_dialog(chat: &mut Chat, dialog_rx: &Receiver<DialogExchange>) -> bool {
    // A host-rendered question (gh #124): one modal at a time, so
    // this waits for both the permission modal and any open dialog.
    // A notification never opens chrome: it notices and answers.
    if chat.world.permission.is_none()
        && chat.world.dialog.is_none()
        && let Ok(exchange) = dialog_rx.try_recv()
    {
        if let lca_protocol::UiDialog::Notify { message, .. } = &exchange.dialog {
            chat.world.notice = Some(message.clone());
            let _ = exchange.respond.send(lca_protocol::DialogAnswer::Notify);
        } else {
            // A select opens with every option matched (an empty
            // filter matches all, like the pickers).
            let matches = match &exchange.dialog {
                lca_protocol::UiDialog::Select { options, .. } => (0..options.len()).collect(),
                _ => Vec::new(),
            };
            chat.world.dialog = Some(DialogModal {
                exchange,
                query: String::new(),
                matches,
                selected: 0,
                input: String::new(),
            });
        }
        return true;
    }
    false
}

/// Re-filter a dialog select after the query changed: a case-folded
/// substring over the options, keeping the highlight in range (the
/// pickers' rule, gh #124). Split into field borrows so the caller can
/// hold the options while updating the filter state.
fn refilter_dialog(
    query: &str,
    matches: &mut Vec<usize>,
    selected: &mut usize,
    options: &[String],
) {
    let needle = query.to_lowercase();
    *matches = options
        .iter()
        .enumerate()
        .filter(|(_, option)| option.to_lowercase().contains(&needle))
        .map(|(index, _)| index)
        .collect();
    if *selected >= matches.len() {
        *selected = matches.len().saturating_sub(1);
    }
}

/// A host-rendered dialog, while open (gh #172): confirm answers
/// on y/n, select filters and picks, input edits one line. Escape
/// always denies (dismisses); the worker is never left hanging.
pub fn handle_dialog(chat: &mut Chat, data: &str, key: Option<&str>) -> Option<Action> {
    use lca_protocol::{DialogAnswer, UiDialog};
    let mut modal = chat.world.dialog.take()?;
    let answer = match &mut modal.exchange.dialog {
        UiDialog::Confirm { .. } => match key {
            Some("y" | "Y" | "enter") => Some(DialogAnswer::Confirm(true)),
            Some("n" | "N" | "escape") => Some(DialogAnswer::Confirm(false)),
            _ => None,
        },
        UiDialog::Select { options, .. } => match key {
            Some("escape") => Some(DialogAnswer::Select(None)),
            Some("enter") => Some(DialogAnswer::Select(
                modal
                    .matches
                    .get(modal.selected)
                    .and_then(|index| options.get(*index))
                    .cloned(),
            )),
            Some("up") => {
                modal.selected = modal.selected.saturating_sub(1);
                None
            }
            Some("down") => {
                modal.selected = (modal.selected + 1).min(modal.matches.len().saturating_sub(1));
                None
            }
            Some("backspace") => {
                modal.query.pop();
                refilter_dialog(
                    &modal.query,
                    &mut modal.matches,
                    &mut modal.selected,
                    options,
                );
                None
            }
            _ => match printable(data) {
                Some(text) => {
                    modal.query.push_str(&text);
                    refilter_dialog(
                        &modal.query,
                        &mut modal.matches,
                        &mut modal.selected,
                        options,
                    );
                    None
                }
                None => None,
            },
        },
        UiDialog::Input { .. } => {
            if let Some(text) = paste_text(data) {
                modal.input.push_str(&text);
                None
            } else {
                match key {
                    Some("escape") => Some(DialogAnswer::Input(None)),
                    Some("enter") => Some(if modal.input.is_empty() {
                        DialogAnswer::Input(None)
                    } else {
                        DialogAnswer::Input(Some(std::mem::take(&mut modal.input)))
                    }),
                    Some("backspace") => {
                        modal.input.pop();
                        None
                    }
                    _ => match printable(data) {
                        Some(text) => {
                            modal.input.push_str(&text);
                            None
                        }
                        None => None,
                    },
                }
            }
        }
        UiDialog::Notify { .. } => Some(DialogAnswer::Notify),
    };
    match answer {
        Some(answer) => {
            let _ = modal.exchange.respond.send(answer);
        }
        None => {
            chat.world.dialog = Some(modal);
        }
    }
    Some(Action::Continue)
}
