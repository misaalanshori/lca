//! The `/settings` project-override row (gh #397), from the picker's side.
//!
//! The file behaviour (the cycle's writes, the byte-identical round trip, the
//! removal of an emptied file) is pinned in
//! `crates/lca-cli/tests/tri_state_override.rs` against the real writer. These
//! rows cover what the interface promises on top of that: which rows offer the
//! tri-state at all, that the cycle asks the host rather than writing itself,
//! that a failed write is reported instead of faked, and that an untrusted
//! project says so rather than pretending its override applied.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::state::ProjectOverride;

/// The writes a test's hook recorded: `(key, value)`, where `None` is a
/// removal - the `inherit` state.
type Writes = Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;

/// One row, optionally project-overridable.
fn row(key: &str, project: Option<ProjectOverride>, values: &[&str]) -> crate::state::SettingRow {
    crate::state::SettingRow {
        section: String::new(),
        key: key.to_string(),
        label: key.to_string(),
        description: String::new(),
        value: "false".to_string(),
        source: "default".to_string(),
        masked: false,
        values: values.iter().map(|value| value.to_string()).collect(),
        project,
    }
}

/// A chat with a picker open over `rows`, and recording hooks.
fn chat_with(rows: Vec<crate::state::SettingRow>, writes: &Writes, trusted: bool) -> Chat {
    let mut chat = chat();
    chat.settings_picker = Some(crate::chat_pickers::SettingsPicker {
        rows,
        selected: 0,
        editing: None,
        query: String::new(),
    });
    let log = writes.clone();
    chat.world.options.hooks.persist_project_setting =
        Some(Arc::new(move |key: &str, value: Option<String>| {
            log.lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push((key.to_string(), value));
            Ok(())
        }));
    chat.world.options.hooks.project_layer_trusted = Some(Arc::new(move || trusted));
    chat
}

// Verifies: gh #397 - the cycle asks the host for **three** states, and
// `inherit` asks for the key's removal (`None`), not for `false`. This is the
// one-way-door fix seen from the interface: the third press must mean "forget
// this here", never "write an opinion that says nothing".
#[test]
fn the_cycle_asks_for_removal_when_it_returns_to_inherit() {
    let writes: Writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut chat = chat_with(
        vec![row(
            "tool.edit_requires_read",
            Some(ProjectOverride::Inherit),
            &["false", "true"],
        )],
        &writes,
        true,
    );

    for _ in 0..3 {
        let mut picker = chat.settings_picker.take().expect("open");
        let _ = chat.cycle_project_override(&mut picker, true);
        chat.settings_picker = Some(picker);
    }
    let log = writes.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(
        log,
        vec![
            (
                "tool.edit_requires_read".to_string(),
                Some("true".to_string())
            ),
            (
                "tool.edit_requires_read".to_string(),
                Some("false".to_string())
            ),
            ("tool.edit_requires_read".to_string(), None),
        ],
        "inherit is the key's absence: true, false, then remove"
    );
}

// Verifies: gh #397 - a row with no project layer is left alone. Cycling it
// would write a key the row never offered to override.
#[test]
fn a_row_without_a_project_layer_is_not_written() {
    let writes: Writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut chat = chat_with(
        vec![row("ui.color", None, &["auto", "never"])],
        &writes,
        true,
    );
    let mut picker = chat.settings_picker.take().expect("open");
    let notice = chat.cycle_project_override(&mut picker, true);
    chat.settings_picker = Some(picker);
    assert!(
        writes.lock().unwrap_or_else(|p| p.into_inner()).is_empty(),
        "nothing was written for a non-overridable row"
    );
    assert!(notice.contains("not a project override"), "{notice}");
}

// Verifies: gh #397 - **an untrusted project says so.** A write into a project
// file the user has not trusted does not take effect (FR-PERM-9), and the
// notice must report that instead of claiming the change applied.
#[test]
fn an_untrusted_project_reports_that_the_override_is_not_live() {
    let writes: Writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut chat = chat_with(
        vec![row(
            "tool.edit_requires_read",
            Some(ProjectOverride::Inherit),
            &["false", "true"],
        )],
        &writes,
        false,
    );
    let mut picker = chat.settings_picker.take().expect("open");
    let notice = chat.cycle_project_override(&mut picker, true);
    chat.settings_picker = Some(picker);
    assert!(
        notice.contains("not trusted"),
        "the notice names the reason the override is inert: {notice}"
    );
    assert!(
        !writes.lock().unwrap_or_else(|p| p.into_inner()).is_empty(),
        "the key was still written to the file - it is the trust bit, not the write, that gates it"
    );
}

// Verifies: gh #397 - a trusted project applies and does **not** disclaim. The
// mirror of the row above: if this notice also said "not trusted", the warning
// would be noise nobody reads.
#[test]
fn a_trusted_project_applies_without_a_disclaimer() {
    let writes: Writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut chat = chat_with(
        vec![row(
            "tool.edit_requires_read",
            Some(ProjectOverride::Inherit),
            &["false", "true"],
        )],
        &writes,
        true,
    );
    let mut picker = chat.settings_picker.take().expect("open");
    let notice = chat.cycle_project_override(&mut picker, true);
    chat.settings_picker = Some(picker);
    assert!(notice.contains("project on"), "{notice}");
    assert!(!notice.contains("not trusted"), "{notice}");
}

// Verifies: gh #397 - a write that fails is reported, and the row keeps showing
// what the file actually says. A picker that swallowed the error would leave
// the user believing an override landed.
#[test]
fn a_failed_write_is_reported_not_faked() {
    let mut chat = chat();
    chat.settings_picker = Some(crate::chat_pickers::SettingsPicker {
        rows: vec![row(
            "tool.edit_requires_read",
            Some(ProjectOverride::Inherit),
            &["false", "true"],
        )],
        selected: 0,
        editing: None,
        query: String::new(),
    });
    let attempts = Arc::new(AtomicUsize::new(0));
    let counted = attempts.clone();
    chat.world.options.hooks.persist_project_setting =
        Some(Arc::new(move |_key: &str, _value: Option<String>| {
            counted.fetch_add(1, Ordering::SeqCst);
            Err("disk is read-only".to_string())
        }));
    chat.world.options.hooks.project_layer_trusted = Some(Arc::new(|| true));

    let mut picker = chat.settings_picker.take().expect("open");
    let notice = chat.cycle_project_override(&mut picker, true);
    chat.settings_picker = Some(picker);
    assert_eq!(attempts.load(Ordering::SeqCst), 1, "one attempt");
    assert!(
        notice.contains("could not write"),
        "the failure is named: {notice}"
    );
}

// Verifies: gh #397 - the reverse step walks the cycle backward, and the notice
// names both the new and the previous state so the user can tell what changed
// without watching the row.
#[test]
fn the_backward_step_and_its_notice() {
    let writes: Writes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut chat = chat_with(
        vec![row(
            "tool.edit_requires_read",
            Some(ProjectOverride::Inherit),
            &["false", "true"],
        )],
        &writes,
        true,
    );
    let mut picker = chat.settings_picker.take().expect("open");
    let notice = chat.cycle_project_override(&mut picker, false);
    chat.settings_picker = Some(picker);
    assert!(
        notice.contains("project off") && notice.contains("was inherit"),
        "the notice says where it went and where it came from: {notice}"
    );
    let log = writes.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(
        log,
        vec![(
            "tool.edit_requires_read".to_string(),
            Some("false".to_string())
        )]
    );
}

// Verifies: gh #397 (NFR-28) - the rendered row carries the state as a word, so
// the tri-state is legible without colour and without decoding the source
// column.
#[test]
fn the_rendered_row_names_the_project_state() {
    for (state, word) in [
        (ProjectOverride::Inherit, "project inherit, "),
        (ProjectOverride::On, "project on, "),
        (ProjectOverride::Off, "project off, "),
    ] {
        let mut chat = chat();
        chat.settings_picker = Some(crate::chat_pickers::SettingsPicker {
            rows: vec![row(
                "tool.edit_requires_read",
                Some(state),
                &["false", "true"],
            )],
            selected: 0,
            editing: None,
            query: String::new(),
        });
        // `viewport` is what the painter draws, including the picker overlay.
        let painted = strip(&chat.viewport(100, 30, 0));
        let text = painted.join("\n");
        assert!(text.contains(word), "the row spells `{word}`:\n{text}");
    }
}
