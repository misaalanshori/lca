//! `Chat`'s overlay visibility predicates (gh #373 gap 6): the side panel's
//! responsive breakpoint, and the focus trap a hidden overlay must not spring.
//!
//! Split from `chat_overlay_tests.rs` under the workspace's 1,200-line file
//! ceiling. The fixtures (`options`, `chat`, `strip`) stay in `chat_tests.rs`
//! and are imported from it.
//!
//! The predicate rows live together because they are **one contract**: the same
//! answer decides the paint and the keyboard. Splitting the paint row from the
//! focus row is how the two halves drift, which is the failure mode this cycle
//! exists to close.

use super::*;
use super::{chat, strip};

// ===========================================================================
// gh #373 gap 6: the side panel's visibility predicate, and the focus trap.
//
// The option exists so a floating HUD can hide itself below a width breakpoint
// **without** its owner rebuilding its options on every resize. The safe half is
// not the paint: a panel hidden in the painter but still owning the keyboard makes
// the app feel possessed. These rows are that half.
// ===========================================================================

/// The width gate the panel uses in these rows.
fn panel_wide_enough(min_cols: u16) -> Box<dyn Fn(u16, u16) -> bool> {
    Box::new(move |cols, _rows| cols >= min_cols)
}

// Verifies: gh #373 gap 6 - **a panel hidden by its own breakpoint does not eat the
// keystroke.**
//
// The predicate is set to "at least 90 columns" and the frame is painted at 60. The
// key then has to reach the editor. Before the focus half existed, `handle_panel`
// answered first and the key vanished into a surface that was not on screen - which
// is precisely the bug the option would introduce if it were implemented in the
// painter alone.
#[test]
fn a_panel_hidden_by_its_breakpoint_does_not_own_the_keyboard() {
    let mut chat = chat();
    // An interactor that swallows everything: without the gate this is what eats the
    // keystroke, and with it the editor gets the key. A panel with no interactor
    // registered answers `None` either way, which is why the first version of this row
    // passed against the defect - the harness, not the fix, was doing the work.
    chat.world.options.ui_events =
        Some(Arc::new(|region: &str, _input: &lca_protocol::UiInput| {
            (region == "panel").then_some((String::new(), lca_protocol::UiEffect::None))
        }));
    chat.panel_visible = Some(panel_wide_enough(90));
    chat.world.panel_open = true;
    // Paint at 60 columns: below the breakpoint, so the panel is not on screen.
    let _ = chat.viewport(60, 24, 0);
    assert!(
        !chat.panel_shown(60, 24),
        "the frame is below the panel's breakpoint"
    );

    chat.handle_key("x");
    assert_eq!(
        chat.editor.text(),
        "x",
        "the key reached the editor instead of a hidden panel"
    );
    assert!(
        chat.world.panel_open,
        "and the panel is still open, merely hidden"
    );
}

// Verifies: gh #373 gap 6 - the same options value shows the panel **above** the
// breakpoint, with no re-construction: two frames, two answers.
//
// This is the property a static `bool` cannot have, and it is why the option is a
// predicate. It also proves the answer follows the *frame's* size rather than a
// stale one.
#[test]
fn the_panel_shows_again_above_its_breakpoint() {
    let mut chat = chat();
    chat.panel_visible = Some(panel_wide_enough(90));
    chat.world.panel_open = true;
    assert!(!chat.panel_shown(60, 24), "hidden when narrow");
    assert!(chat.panel_shown(120, 24), "shown when wide");
    assert!(chat.panel_shown(90, 24), "and the breakpoint is inclusive");
}

// Verifies: gh #373 gap 6 - **the key gate reads the size the last frame painted at,
// not the terminal's current size.**
//
// Input dispatch has no size argument, so a second guess here would be a second
// answer. The frame at 60 columns is what the reader is looking at, and it says the
// panel is hidden; a later resize that has not been painted yet must not flip the
// gate underneath the reader.
#[test]
fn the_key_gate_reads_the_size_of_the_frame_on_screen() {
    let mut chat = chat();
    chat.panel_visible = Some(panel_wide_enough(90));
    chat.world.panel_open = true;
    let _ = chat.viewport(60, 24, 0);
    assert!(!chat.panel_shown_now(), "hidden, per the painted frame");

    // The terminal grew, but nothing has painted at the new size yet. `world.resize` is
    // what the host calls on SIGWINCH, and it is deliberately *not* trusted here: the
    // reader is still looking at the 60-column frame, so a key must still be judged
    // against it. This is the half that distinguishes "the painted frame" from "the
    // terminal's current size" - without it the row cannot tell the two apart, which is
    // how the first version of this row passed against the wrong implementation.
    chat.world.resize(120, 24);
    assert_eq!(chat.world.size, (120, 24), "the terminal did grow");
    assert!(
        !chat.panel_shown_now(),
        "the gate still answers for the frame on screen"
    );

    // One frame at the new size, and the gate follows.
    let _ = chat.viewport(120, 24, 0);
    assert!(chat.panel_shown_now(), "now the frame agrees it is visible");
}

// Verifies: gh #373 gap 6 (control) - **no predicate means always shown**, so every
// existing host keeps exactly the behaviour it had.
#[test]
fn a_chat_with_no_panel_predicate_shows_the_panel_while_open() {
    let mut chat = chat();
    chat.world.panel_open = true;
    assert!(chat.panel_shown(40, 24), "no predicate: shown at any size");
    assert!(chat.panel_shown_now(), "and the key gate agrees");
    chat.world.panel_open = false;
    assert!(!chat.panel_shown(200, 60), "and closed is closed");
}

// Verifies: gh #373 gap 6 - a hidden panel is not painted either.
//
// The painter half. A panel below its breakpoint must contribute no rows, or the
// predicate would only move the defect: the box would still take the right-hand
// columns while its keys went to the editor.
#[test]
fn a_panel_below_its_breakpoint_paints_nothing() {
    let mut chat = chat();
    chat.world.options.render_regions = Some(Arc::new(|region: &str| {
        if region == "panel" {
            vec![(
                "panel".to_string(),
                lca_protocol::WidgetTree {
                    nodes: vec![lca_protocol::Widget::Text {
                        content: "PANEL-MARKER".into(),
                        role: "default".into(),
                    }],
                },
            )]
        } else {
            Vec::new()
        }
    }));
    chat.panel_visible = Some(panel_wide_enough(90));
    chat.world.panel_open = true;
    // The panel is composited over the viewport, so the fullscreen path is the one
    // that paints it - the same path `compose_overlays` serves.
    chat.screen_mode = true;

    let narrow = strip(&chat.viewport(60, 24, 0)).join("\n");
    assert!(
        !narrow.contains("PANEL-MARKER"),
        "no panel rows below the breakpoint:\n{narrow}"
    );
    let wide = strip(&chat.viewport(120, 24, 0)).join("\n");
    assert!(
        wide.contains("PANEL-MARKER"),
        "and the panel paints above it:\n{wide}"
    );
}

// ===========================================================================
// gh #373 gap 5: layer order. The paint order and the key order must be the
// SAME order, or the surface that takes a keystroke is not the one on screen.
// ===========================================================================

/// A `/model` picker with rows, so it is genuinely open.
fn picker_up(chat: &mut Chat) {
    chat.world.options.models = vec![crate::ModelRow::from((
        "alpha".to_string(),
        "alpha".to_string(),
    ))];
    chat.open_model_picker();
    assert!(chat.model_picker.is_some(), "the picker is open");
}

/// A permission prompt over whatever is already up.
fn permission_up(chat: &mut Chat, action: &str) {
    chat.world.permission = Some(crate::state::PermissionModal {
        action: action.to_string(),
        respond: None,
        deadline: None,
    });
}

// Verifies: gh #373 gap 5 - **a permission prompt raised over an open picker owns
// the keyboard AND is painted.**
//
// These two disagree today, which is the defect: `compose_pickers` returns early, so
// the prompt is never drawn, while `handle_modal_key` runs before the picker and takes
// the key. The user sees a picker, types into a picker, and gets nothing - and nothing
// on screen says why. This row asserts the keyboard owner; the paint half is
// `the_prompt_over_a_picker_is_actually_painted`.
#[test]
fn a_permission_prompt_over_a_picker_owns_the_keyboard() {
    let mut chat = chat();
    picker_up(&mut chat);
    permission_up(&mut chat, "shell cargo test");

    // `d` denies, which resolves the prompt (`handle_permission`'s table). If the
    // picker had the key instead it would be typed into its search box.
    chat.handle_key("d");
    assert!(
        chat.world.permission.is_none(),
        "the prompt took the key and resolved"
    );
    assert!(
        chat.model_picker.is_some(),
        "and the picker beneath it is still open, not dismissed by the answer"
    );
}

// Verifies: gh #373 gap 5 - **the top layer is the one painted.**
//
// The same stack answers "what is on screen". Before the layer order was shared, the
// prompt was invisible: a user answering a question they could not see.
#[test]
fn the_prompt_over_a_picker_is_actually_painted() {
    let mut chat = chat();
    chat.screen_mode = true;
    picker_up(&mut chat);
    permission_up(&mut chat, "shell cargo test");
    // `render()` is the document only - overlays are composited by the viewport path
    // (main screen and fullscreen both reach `compose_overlays` through it).
    let painted = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(
        painted.contains("cargo test"),
        "the prompt's action is on screen:\n{painted}"
    );
}

// --- the matrix: which surface receives the key, surface by surface -------------

/// Whether the editor owns the keyboard right now.
fn editor_owns(chat: &Chat) -> bool {
    chat.top_layer().is_none()
}

// Verifies: gh #373 gap 5 - **an empty stack routes to the editor.**
//
// The no-vanish row: with nothing raised, a printable key lands in the prompt. If this
// ever fails, a keystroke goes nowhere with nothing on screen to explain it.
#[test]
fn an_empty_stack_routes_the_key_to_the_editor() {
    let mut chat = chat();
    assert!(editor_owns(&chat));
    chat.handle_key("q");
    assert_eq!(chat.editor.text(), "q", "the editor took the key");
}

// Verifies: gh #373 gap 5 - **picker → modal → picker**: the prompt takes the key
// while it is up, and the picker beneath it takes the next one after the prompt
// resolves. The second half is what "no stranded focus" means.
#[test]
fn a_prompt_over_a_picker_hands_the_key_back_when_it_closes() {
    let mut chat = chat();
    picker_up(&mut chat);
    permission_up(&mut chat, "shell cargo test");

    chat.handle_key("d");
    assert!(chat.world.permission.is_none(), "the prompt resolved");
    assert_eq!(
        chat.top_layer(),
        Some(crate::layer_stack::Layer::Picker),
        "and the picker is what takes the next key"
    );
    // A printable key now filters the picker rather than reaching the editor.
    chat.handle_key("z");
    assert!(
        !chat.editor.text().contains('z'),
        "the picker took it, not the editor"
    );
}

// Verifies: gh #373 gap 5 - **modal → search**: closing the modal leaves the search
// readout owning the keyboard, not the editor.
#[test]
fn closing_a_modal_over_a_search_leaves_the_search_in_charge() {
    let mut chat = chat();
    chat.search = Some("needle".to_string());
    permission_up(&mut chat, "shell cargo test");
    assert_eq!(chat.top_layer(), Some(crate::layer_stack::Layer::Modal));

    chat.handle_key("d");
    assert!(chat.world.permission.is_none());
    assert_eq!(
        chat.top_layer(),
        Some(crate::layer_stack::Layer::Search),
        "the search is what was underneath"
    );
}

// Verifies: gh #373 gap 5 - **closing the top when the stack is one deep** returns the
// editor, which is the case every picker's own Escape takes.
#[test]
fn closing_the_only_layer_returns_the_editor() {
    let mut chat = chat();
    picker_up(&mut chat);
    assert_eq!(chat.top_layer(), Some(crate::layer_stack::Layer::Picker));
    let _ = chat.handle_picker_key("", Some("escape"));
    assert!(chat.model_picker.is_none(), "the picker closed");
    assert!(editor_owns(&chat), "and the editor has the keyboard again");
    chat.handle_key("w");
    assert_eq!(chat.editor.text(), "w");
}

// Verifies: gh #373 gap 5 - **the panel never takes a key away from a layer above it,
// and never takes one from the editor either.** It is composited, not focused.
#[test]
fn the_panel_occupies_a_rung_without_owning_the_keyboard() {
    let mut chat = chat();
    chat.world.panel_open = true;
    assert!(editor_owns(&chat), "nothing keyboard-owning is raised");
    chat.handle_key("p");
    assert_eq!(chat.editor.text(), "p", "the editor still types");
}

// Verifies: gh #373 gap 5 - **a /bug inspection screen is a modal layer.**
//
// It takes keys through `handle_modal_key` and is painted by `compose_modals`, but it
// was absent from `UiState::modal_active`, so no predicate that asked about modals saw
// it. Derived from the union of "takes keys" and "is painted", the layer can no longer
// miss a surface.
#[test]
fn a_bug_review_screen_is_a_modal_layer() {
    let mut chat = chat();
    chat.world.bug_review = Some(crate::state::BugReview {
        inspection: "bug bundle - review before sharing".to_string(),
        destination: "/tmp/bundle.zip".to_string(),
        resolve: std::sync::Arc::new(|_write| String::new()),
    });
    assert_eq!(chat.top_layer(), Some(crate::layer_stack::Layer::Modal));
    assert!(
        !editor_owns(&chat),
        "so a stray key does not reach the editor"
    );
}

// Verifies: gh #373 gap 5 - **a click goes to the layer on top, not to the picker
// underneath it.**
//
// The pointer and the keyboard were disagreeing: `handle_mouse_event` asked
// `picker_open()` first, so a prompt over a picker received no clicks while taking
// every key. Same stack, same answer, so the two cannot split again.
#[test]
fn a_click_on_a_covered_picker_does_not_reach_it() {
    let mut chat = chat();
    chat.screen_mode = true;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    chat.world.options.ui_events = Some(std::sync::Arc::new(
        move |region: &str, input: &lca_protocol::UiInput| {
            log.lock()
                .unwrap_or_else(|p| p.into_inner())
                .push((region.to_string(), format!("{input:?}")));
            None
        },
    ));
    picker_up(&mut chat);
    permission_up(&mut chat, "shell cargo test");

    assert_eq!(chat.top_layer(), Some(crate::layer_stack::Layer::Modal));
    assert!(
        chat.picker_covered(),
        "the picker is underneath, so it must not answer the click"
    );
    // The routing decision itself, without a live screen: a covered picker is not
    // `picker_open()`, which is the predicate every mouse path asks.
    assert!(
        !chat.picker_open(),
        "a covered picker does not own the pointer or the keyboard"
    );
}

// Verifies: gh #373 gap 5 - **when a picker and an extension modal are both up, the
// modal is in charge on the key path AND the paint path.**
//
// This is the reachable "both raised" case: an extension answers a picker key with
// `OpenModal`, so `modal_open` and a picker are both set. There is no insertion order
// to recover — both were raised the same frame — so the question is which precedence is
// *coherent*, and the answer is fixed by the key path, which has always dispatched
// `handle_modal_key` before `handle_picker_key`. The paint now agrees instead of
// drawing the list over the question.
#[test]
fn a_picker_and_an_extension_modal_together_are_the_modal_on_both_paths() {
    let mut chat = chat();
    chat.screen_mode = true;
    picker_up(&mut chat);
    chat.world.modal_open = true;

    assert_eq!(
        chat.top_layer(),
        Some(crate::layer_stack::Layer::Modal),
        "the modal is the layer in charge, which is also who the key path asks first"
    );
    // The key path and the paint path agree: the picker is covered, so it is not the
    // one answering, and the modal's frame is what the reader sees.
    assert!(chat.picker_covered(), "the picker is beneath the question");
    assert!(
        !chat.picker_open(),
        "so a key reaches the modal, exactly as `handle_modal_key`-first already said"
    );
    let painted = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(
        !painted.contains("alpha"),
        "the covered picker's rows are not painted over the question:\n{painted}"
    );
}
