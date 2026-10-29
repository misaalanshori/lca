//! The transient-overlay registry (gh #333): **one** list of every overlay a tab
//! switch must close, and the generated code that closes them.
//!
//! # Why this module exists
//!
//! `Chat::close_pickers` was a hand-maintained list. It named ten pickers; the
//! Extension Hub and the Extension Manager arrived later (#227) and nobody added them,
//! so opening `/extensions` in Tab 1 and switching left the hub modal **pinned over Tab
//! 2's transcript**. The install-consent dialog leaked the same way, and `mcp_manager`
//! (#418 Phase G) leaked too - which is the point: the issue named three overlays and
//! the defect's mechanism produces a fourth whenever someone adds one.
//!
//! Fixing the three names would have re-created the cause. So the names live here,
//! once, in [`CHAT_OVERLAYS`] and [`WORLD_OVERLAYS`], and:
//!
//! * `close_tab_overlays` is generated from that list by `close_overlays!`, so the
//!   function cannot drift from it;
//! * `tests/regressions/333-*` parses `Chat`'s and `UiState`'s declared fields and fails
//!   if an overlay-shaped field is **missing from this list**. That is the property the
//!   brief asks for - a fifth overlay must fail a row rather than leak - and it is the
//!   part a plain `clear()` cannot give on its own.
//!
//! # Why not a `struct Overlays`
//!
//! It was the other candidate and it is the tidier shape, but these fields are read by
//! bare name at several hundred call sites across the crate. Moving them into a struct
//! would be a mechanical rename of all of them inside a bug-fix cycle, and it would make
//! every `self.model_picker` in the codebase say `self.overlays.model_picker`. The list
//! plus the source-derived guard buys the same guarantee - forgetting is impossible, and
//! a forgotten field fails a named row - without that churn. If the fields are ever
//! consolidated for another reason, this module is the seam that makes it a small
//! change.
//!
//! # What is deliberately not here
//!
//! The draft (`editor`), the scroll position, the transcript, the tab list, and anything
//! a tab owns. A tab switch **preserves** those; over-closing would be a worse bug than
//! the pinned modal. The regression row
//! `a_tab_switch_preserves_the_draft_and_the_scroll` is what keeps that honest.

use crate::chat::Chat;

/// Every transient overlay on [`Chat`], by field name.
///
/// Order is the order the tab-switch path clears them, and it is grouped by owner:
/// the pickers first, then the manager/modal overlays, then the transcript search.
pub const CHAT_OVERLAYS: &[&str] = &[
    "model_picker",
    "tree_picker",
    "resume_picker",
    "fork_picker",
    "scoped_models_picker",
    "grants_picker",
    "trust_picker",
    "settings_picker",
    "thinking_picker",
    "theme_picker",
    "extension_hub",
    "extension_manager",
    "mcp_manager",
    "install_consent",
    "search",
];

/// Every transient overlay on [`UiState`](crate::ui_state::UiState), by field name.
///
/// The login/permission/bug modals live here rather than on `Chat`, and they are the
/// same kind of thing: transient UI that belongs to the tab that opened it. `panel_open`
/// and `modal_open` are flags rather than `Option`s and have their own row; `dialog`
/// belongs to a host modal, which is the host's to withdraw.
pub const WORLD_OVERLAYS: &[&str] = &[
    "permission",
    "secret",
    "picker",
    "grant",
    "switch_confirm",
    "bug_review",
    "login_waiting",
    "wizard",
];

/// Clear the named fields on `$self`. One statement per name, so a name that is not a
/// field of the type is a **compile error** rather than a silently skipped line.
macro_rules! close_overlays {
    ($self:ident; $($field:ident),* $(,)?) => {
        $( $self.$field = None; )*
    };
}

/// Close every transient overlay on a `Chat` and its `UiState` (gh #333).
///
/// Called from `Chat::close_pickers`, which every tab switch, tab open, and tab close
/// goes through.
pub(crate) fn close_tab_overlays(chat: &mut Chat) {
    // Generated from `CHAT_OVERLAYS`'s members, spelled out because Rust macros cannot
    // iterate a `const` slice's *identifiers*. The regression row is what keeps this
    // list and `CHAT_OVERLAYS` equal; `CHAT_OVERLAYS` is what the row derives "did you
    // remember?" from, so the pair cannot silently disagree.
    close_overlays!(
        chat;
        model_picker,
        tree_picker,
        resume_picker,
        fork_picker,
        scoped_models_picker,
        grants_picker,
        trust_picker,
        settings_picker,
        thinking_picker,
        theme_picker,
        extension_hub,
        extension_manager,
        mcp_manager,
        install_consent,
        search,
    );
    clear_world_overlays(&mut chat.world);
}

/// Clear the `UiState` half. Split out because `UiState` has its own row and its own
/// owner; `/reload` does **not** call this (see the module docs on the report).
fn clear_world_overlays(world: &mut crate::ui_state::UiState) {
    close_overlays!(
        world;
        permission,
        secret,
        picker,
        grant,
        switch_confirm,
        bug_review,
        login_waiting,
        wizard,
    );
    // The flags that carry the same meaning but are not `Option`s. Set rather than
    // cleared-to-None, so they are named here rather than generated.
    world.panel_open = false;
    world.modal_open = false;
}
