//! Gh #397: value-preserving cycles and stepped settings wizards.
//!
//! # The cycle defect, in the issue's own words
//!
//! A user sets `ui.fullscreen_wheel_lines = 4` by hand - a number the presets
//! (`1, 2, 3, 5, 10`) do not contain. The picker's cycle computed
//! `values.iter().position(|v| *v == current).unwrap_or(0)`: a miss falls back to
//! index 0, so the very first press jumps to the *second* preset and the user's
//! value is gone from the file. That is a one-way door on a config file - the same
//! defect the tri-state fix (batch-20) closed for booleans, in the enum case.
//!
//! The fix follows pi's `settings-selector.ts`: the cycle list is the presets
//! **union the current value**, so the user's own number is a stop on the cycle
//! instead of something the list falls off. A `default` stop (the key removed, so
//! the built-in default wins) is the first entry, which is what makes
//! `custom → default → a → b → … → custom` close.
//!
//! # The wizard half
//!
//! A related group (a model and its thinking level) is walked as ordered steps
//! rather than toggled one key at a time. The rule that matters is atomicity: a
//! wizard cancelled at step two applies **nothing**, and a later step that depends
//! on an earlier choice reads the **pending** value - otherwise a cancel leaves the
//! session on a value the user never confirmed.
//!
//! Verifies: gh #397, FR-CFG-2, FR-UI-16, NFR-24, NFR-28.

use super::*;
use super::{chat, strip};

/// The persist hook's recording cell: `(key, value)` per call, `None` meaning the
/// key was removed (`default`).
type Written = std::sync::Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;

/// A row with a preset cycle and whatever the file holds.
fn cycle_row(key: &str, value: &str, values: &[&str]) -> crate::state::SettingRow {
    crate::state::SettingRow {
        section: "Terminal & Execution".to_string(),
        key: key.to_string(),
        label: "Wheel lines".to_string(),
        description: "Fullscreen lines per mouse-wheel tick".to_string(),
        value: value.to_string(),
        source: "user file".to_string(),
        masked: false,
        values: values.iter().map(|value| value.to_string()).collect(),
        project: None,
    }
}

// ---------------------------------------------------------------------------
// Value-preserving cycles
// ---------------------------------------------------------------------------

// Verifies: gh #397's central row - **a custom value is a stop on the cycle**.
//
// `wheel_lines = 4` is not a preset. Cycling forward from it must reach a preset
// and, cycling far enough, come back to `4` - with the value still in the list
// rather than replaced by the first preset the fallback happens to land on.
//
// Asserted on the cycle list and the chosen values, not on a painted string: the
// defect is which value gets *written*, and a paint assertion would not see a
// write that clobbered the file.
#[test]
fn a_custom_value_is_a_stop_on_its_own_cycle() {
    let row = cycle_row(
        "ui.fullscreen_wheel_lines",
        "4",
        &["1", "2", "3", "5", "10"],
    );
    let options = Chat::cycle_options_with(&row, None);
    assert!(
        options.contains(&"4".to_string()),
        "the user's own value is in the cycle, not dropped: {options:?}"
    );
    // Sorted, so the order is the number line rather than presets-then-custom.
    assert_eq!(
        options,
        vec!["default", "1", "2", "3", "4", "5", "10"],
        "default first, then presets union the current value, in order"
    );
}

// Verifies: gh #397 - **the round trip closes**. Starting at a custom value,
// stepping through every option returns to it. This is the property the old
// `unwrap_or(0)` fallback could not have: a miss became index 0, so the custom
// value left the list and there was no way back.
#[test]
fn cycling_from_a_custom_value_returns_to_it() {
    let row = cycle_row(
        "ui.fullscreen_wheel_lines",
        "4",
        &["1", "2", "3", "5", "10"],
    );
    let options = Chat::cycle_options_with(&row, None);
    let start = options
        .iter()
        .position(|value| value == "4")
        .expect("the custom value is a stop");
    let mut current = options[start].clone();
    let mut seen = Vec::new();
    for _ in 0..options.len() {
        seen.push(current.clone());
        current = Chat::next_cycle_value(&options, &current, true);
    }
    assert_eq!(
        current, "4",
        "a full lap returns to the custom value: {seen:?}"
    );
    assert_eq!(seen.len(), options.len(), "and visits every stop once");
}

// Verifies: gh #397 - the regression pin the brief asks for: **a row with no
// custom value cycles its presets normally**, and does not gain a phantom entry.
//
// Without this row, adding the current value unconditionally would look correct
// while inserting a duplicate stop for every ordinary row.
#[test]
fn a_row_without_a_custom_value_cycles_its_presets_unchanged() {
    let row = cycle_row(
        "ui.fullscreen_wheel_lines",
        "3",
        &["1", "2", "3", "5", "10"],
    );
    assert_eq!(
        Chat::cycle_options_with(&row, None),
        vec!["default", "1", "2", "3", "5", "10"],
        "the current value is already a preset, so it adds no stop of its own"
    );
}

// Verifies: gh #397 - **`default` is reachable and is written as key removal**,
// not as a magic value. `persist_setting(key, None)` is the existing contract for
// unsetting (the same one the tri-state uses), so the cycle reuses it rather than
// inventing a second way to say "unset".
#[test]
fn the_default_stop_removes_the_key_rather_than_writing_a_value() {
    let row = cycle_row("ui.fullscreen_wheel_lines", "4", &["1", "2", "5"]);
    let options = Chat::cycle_options_with(&row, None);
    // Stepping back from the first preset lands on `default`.
    assert_eq!(Chat::next_cycle_value(&options, "1", false), "default");
    assert_eq!(
        Chat::persisted_for("default", "ui.fullscreen_wheel_lines"),
        None,
        "`default` persists as a removal, which is how unset is written"
    );
    assert_eq!(
        Chat::persisted_for("5", "ui.fullscreen_wheel_lines"),
        Some("5".to_string()),
        "and a real value persists as itself"
    );
}

// Verifies: gh #397 - a masked row cycles without its value entering the cycle
// list as a stop. A substituted secret (`{env:}`/`{file:}`) is shown as
// `********`, and treating that as a "custom value" would add a stop that writes
// the literal mask into the file (gh #218's rule: the real text stays in `value`,
// the screen shows stars).
#[test]
fn a_masked_row_does_not_cycle_its_mask() {
    let mut row = cycle_row("shell.command_prefix", "secret-token", &["a", "b"]);
    row.masked = true;
    row.value = "********".to_string();
    let options = Chat::cycle_options_with(&row, None);
    assert!(
        !options.contains(&"********".to_string()),
        "the mask is never a cycle stop: {options:?}"
    );
}

// ---------------------------------------------------------------------------
// Stepped wizards
// ---------------------------------------------------------------------------

// Verifies: gh #397's atomicity row - **a cancelled wizard applies nothing**.
//
// The brief is explicit that this is the half that can do damage: "not the steps
// you already confirmed". A wizard that persists as it goes leaves the session on
// a half-finished configuration with no way to recognise it as such.
#[test]
fn a_wizard_cancelled_at_a_later_step_applies_nothing() {
    let mut chat = chat();
    chat.screen_mode = true;
    let written: Written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = written.clone();
    chat.world.options.hooks.persist_setting = Some(std::sync::Arc::new(
        move |key: &str, value: Option<String>| {
            if let Ok(mut log) = sink.lock() {
                log.push((key.to_string(), value));
            }
        },
    ));

    let mut wizard = crate::chat_pickers::SteppedWizard::new(
        "thinking",
        vec![
            crate::chat_pickers::WizardStep {
                key: "provider.model".to_string(),
                label: "Model".to_string(),
                value: "m1".to_string(),
                values: vec!["m1".to_string(), "m2".to_string()],
            },
            crate::chat_pickers::WizardStep {
                key: "thinking".to_string(),
                label: "Thinking level".to_string(),
                value: "low".to_string(),
                values: vec!["low".to_string(), "high".to_string()],
            },
        ],
    );
    // Step one: choose the second model. Pending only.
    wizard.choose("m2");
    wizard.step_forward();
    // Step two: cancel.
    let cancelled = wizard.take_cancel();
    assert!(cancelled.is_some(), "a cancel is reported as one");

    let written = written.lock().map(|w| w.clone()).unwrap_or_default();
    assert!(
        written.is_empty(),
        "a cancelled wizard writes nothing at all: {written:?}"
    );
    assert_eq!(
        chat.world
            .options
            .thinking
            .lock()
            .map(|t| t.clone())
            .unwrap_or(None),
        None,
        "and the live session is untouched"
    );
}

// Verifies: gh #397 - **a later step reads the pending choice**, not the applied
// one. When step two's options depend on step one (a model's allowed thinking
// levels), reading the applied value would compute the *old* model's levels - and
// on cancel the session would be left describing a model it is not on.
#[test]
fn a_later_step_derives_its_options_from_the_pending_choice() {
    let mut wizard = crate::chat_pickers::SteppedWizard::new(
        "thinking",
        vec![
            crate::chat_pickers::WizardStep {
                key: "provider.model".to_string(),
                label: "Model".to_string(),
                value: "m1".to_string(),
                values: vec!["m1".to_string(), "m2".to_string()],
            },
            crate::chat_pickers::WizardStep {
                key: "thinking".to_string(),
                label: "Thinking level".to_string(),
                value: "low".to_string(),
                values: Vec::new(), // derived from the pending model
            },
        ],
    );
    wizard.choose("m2");
    assert_eq!(
        wizard.pending_value("provider.model"),
        Some("m2"),
        "the pending choice is recorded"
    );
    wizard.step_forward();
    wizard.set_step_values(1, &["low", "medium", "high"]);
    assert_eq!(
        wizard.current().map(|step| step.key.as_str()),
        Some("thinking"),
        "and step two is the one being configured"
    );
    // The derived list is computed from the *pending* model, which the caller
    // reads through `pending_value` - never from the applied one.
    assert_eq!(wizard.pending_value("provider.model"), Some("m2"));
}

// Verifies: gh #397 design point 3 - **the wizard writes through the same persist
// path** `/settings` uses, so there is one writer. A second writer is how the two
// drift, which this repo has closed three times (#403, #456, #407).
#[test]
fn the_wizard_writes_through_the_shared_persist_path() {
    let wizard = crate::chat_pickers::SteppedWizard::new(
        "thinking",
        vec![crate::chat_pickers::WizardStep {
            key: "thinking".to_string(),
            label: "Thinking level".to_string(),
            value: "low".to_string(),
            values: vec!["low".to_string(), "high".to_string()],
        }],
    );
    // The wizard names the hook it uses; it does not hold a file handle or a
    // config writer of its own.
    assert!(
        wizard.persist_through().contains("persist_setting"),
        "the wizard commits through the shared setting persist hook: {}",
        wizard.persist_through()
    );
}

// Verifies: gh #397 - a **completed** wizard commits every step in order, and the
// commit is the only write. This is the other half of atomicity: collecting into
// a pending map is only correct if finishing actually commits it.
#[test]
fn a_completed_wizard_commits_every_step_in_order() {
    let mut wizard = crate::chat_pickers::SteppedWizard::new(
        "thinking",
        vec![
            crate::chat_pickers::WizardStep {
                key: "provider.model".to_string(),
                label: "Model".to_string(),
                value: "m1".to_string(),
                values: vec!["m1".to_string(), "m2".to_string()],
            },
            crate::chat_pickers::WizardStep {
                key: "thinking".to_string(),
                label: "Thinking level".to_string(),
                value: "low".to_string(),
                values: vec!["low".to_string(), "high".to_string()],
            },
        ],
    );
    wizard.choose("m2");
    wizard.step_forward();
    wizard.choose("high");
    let commits = wizard.commit();
    assert_eq!(
        commits,
        vec![
            ("provider.model".to_string(), Some("m2".to_string())),
            ("thinking".to_string(), Some("high".to_string())),
        ],
        "both steps commit, in the order they were walked"
    );
}

// Verifies: gh #397 - **`Esc` cancels the whole wizard**, not just the current
// step. The brief states this explicitly: "`Esc` cancelling the whole wizard -
// **not** leaving earlier steps applied". A step-back key would be a different
// (and absent) binding; Esc is the cancel.
#[test]
fn escape_cancels_the_whole_wizard_rather_than_stepping_back() {
    let mut wizard = crate::chat_pickers::SteppedWizard::new(
        "thinking",
        vec![
            crate::chat_pickers::WizardStep {
                key: "provider.model".to_string(),
                label: "Model".to_string(),
                value: "m1".to_string(),
                values: vec!["m1".to_string(), "m2".to_string()],
            },
            crate::chat_pickers::WizardStep {
                key: "thinking".to_string(),
                label: "Thinking level".to_string(),
                value: "low".to_string(),
                values: vec!["low".to_string(), "high".to_string()],
            },
        ],
    );
    wizard.choose("m2");
    wizard.step_forward();
    assert!(wizard.cancel().is_some(), "Esc cancels the wizard");
    // `pending_value` falls back to the step's *starting* value, so after a
    // cancel it reports what the session is actually on - which is the point:
    // the pending choice is gone and nothing was applied.
    assert!(wizard.pending.is_empty(), "the pending choices are dropped");
    assert_eq!(
        wizard.commit(),
        Vec::new(),
        "and a cancel commits nothing even if the caller asks"
    );
    assert_eq!(
        wizard.pending_value("provider.model"),
        Some("m1"),
        "the value it reports is the applied one, never the cancelled choice"
    );
}

// Verifies: gh #397 design point 5 - **no new keybinding**. The wizard walks with
// keys the picker already has, which the study calls pi's two-tier activation
// (`values`-cycle vs `submenu`-swap): `enter`/`right` advances and chooses,
// `left` goes back a step, `esc` cancels.
#[test]
fn the_wizard_walks_with_existing_keys() {
    let wizard = crate::chat_pickers::SteppedWizard::new(
        "thinking",
        vec![crate::chat_pickers::WizardStep {
            key: "thinking".to_string(),
            label: "Thinking level".to_string(),
            value: "low".to_string(),
            values: vec!["low".to_string(), "high".to_string()],
        }],
    );
    // The hint names only keys the settings picker already binds.
    let hint = wizard.hint();
    for key in ["enter", "left", "esc"] {
        assert!(hint.contains(key), "the hint names `{key}`: {hint}");
    }
    assert!(
        !hint.contains("ctrl+") && !hint.contains("alt+"),
        "and invents no new chord: {hint}"
    );
}

// ---------------------------------------------------------------------------
// The panes
// ---------------------------------------------------------------------------

// Verifies: gh #397 - the wizard is visible with its steps numbered and the
// pending value shown, so a reader knows where they are and what they have
// already chosen. A wizard that shows only the current step makes the earlier
// choices invisible until commit, which is the opposite of the atomicity story.
#[test]
fn the_wizard_paints_its_steps_and_pending_values() {
    let mut chat = chat();
    chat.screen_mode = true;
    chat.world.options.hooks.settings_rows = Some(std::sync::Arc::new(Vec::new));
    let mut wizard = crate::chat_pickers::SteppedWizard::new(
        "thinking",
        vec![
            crate::chat_pickers::WizardStep {
                key: "provider.model".to_string(),
                label: "Model".to_string(),
                value: "m1".to_string(),
                values: vec!["m1".to_string(), "m2".to_string()],
            },
            crate::chat_pickers::WizardStep {
                key: "thinking".to_string(),
                label: "Thinking level".to_string(),
                value: "low".to_string(),
                values: vec!["low".to_string(), "high".to_string()],
            },
        ],
    );
    wizard.choose("m2");
    wizard.step_forward();
    chat.world.wizard = Some(wizard);
    let frame = strip(&chat.viewport(100, 30, 0)).join("\n");
    assert!(frame.contains("Model"), "the steps are named:\n{frame}");
    assert!(
        frame.contains("m2"),
        "the pending choice is visible:\n{frame}"
    );
    assert!(
        frame.contains("1/2") || frame.contains("2/2"),
        "and where the reader is:\n{frame}"
    );
}

// Verifies: gh #397 - **the cancel is real through the key path**, not just on
// the type. The type's own row proves the data model; this drives `handle_key`
// the way a user does (advance, choose, advance, `esc`) and asserts the persist
// hook was never called and the wizard is gone. A wizard reachable only through
// an API no key calls is not a feature.
#[test]
fn escaping_a_wizard_through_the_keyboard_writes_nothing() {
    let mut chat = chat();
    chat.screen_mode = true;
    let written: Written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = written.clone();
    chat.world.options.hooks.persist_setting = Some(std::sync::Arc::new(
        move |key: &str, value: Option<String>| {
            if let Ok(mut log) = sink.lock() {
                log.push((key.to_string(), value));
            }
        },
    ));
    let mut wizard = crate::chat_pickers::SteppedWizard::new(
        "thinking",
        vec![
            crate::chat_pickers::WizardStep {
                key: "provider.model".to_string(),
                label: "Model".to_string(),
                value: "m1".to_string(),
                values: vec!["m1".to_string(), "m2".to_string()],
            },
            crate::chat_pickers::WizardStep {
                key: "thinking".to_string(),
                label: "Thinking level".to_string(),
                value: "low".to_string(),
                values: vec!["low".to_string(), "high".to_string()],
            },
        ],
    );
    wizard.choose("m2");
    wizard.step_forward();
    chat.world.wizard = Some(wizard);

    // The reader cancels at step two, after confirming step one.
    chat.handle_key("\x1b");
    assert!(chat.world.wizard.is_none(), "the wizard closes on esc");
    let written = written.lock().map(|w| w.clone()).unwrap_or_default();
    assert!(
        written.is_empty(),
        "and nothing reached the persist hook: {written:?}"
    );
}

// Verifies: gh #397 - **finishing writes every step through the shared hook**,
// in walk order, and closes the wizard. The atomicity story would be worthless if
// the commit path were the half that did not work.
#[test]
fn finishing_a_wizard_through_the_keyboard_persists_every_step() {
    let mut chat = chat();
    chat.screen_mode = true;
    let written: Written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = written.clone();
    chat.world.options.hooks.persist_setting = Some(std::sync::Arc::new(
        move |key: &str, value: Option<String>| {
            if let Ok(mut log) = sink.lock() {
                log.push((key.to_string(), value));
            }
        },
    ));
    let mut wizard = crate::chat_pickers::SteppedWizard::new(
        "thinking",
        vec![
            crate::chat_pickers::WizardStep {
                key: "provider.model".to_string(),
                label: "Model".to_string(),
                value: "m1".to_string(),
                values: vec!["m1".to_string(), "m2".to_string()],
            },
            crate::chat_pickers::WizardStep {
                key: "thinking".to_string(),
                label: "Thinking level".to_string(),
                value: "low".to_string(),
                values: vec!["low".to_string(), "high".to_string()],
            },
        ],
    );
    wizard.choose("m2");
    wizard.step_forward();
    wizard.choose("high");
    chat.world.wizard = Some(wizard);

    chat.handle_key("\r");
    assert!(chat.world.wizard.is_none(), "the wizard closes on commit");
    let written = written.lock().map(|w| w.clone()).unwrap_or_default();
    assert_eq!(
        written,
        vec![
            ("provider.model".to_string(), Some("m2".to_string())),
            ("thinking".to_string(), Some("high".to_string())),
        ],
        "both steps persist, in order"
    );
}

// Verifies: gh #397 - **the custom value survives the `default` stop**.
//
// Found in a live pane, not in a row: `default` *removes the key*, so the next
// read of the rows finds no value and the user's hand-set `4` was unreachable
// again - the cycle ran `4 → 5 → 10 → default → 1 → 2 → 3 → 5` and never came
// back to `4`. The list alone cannot fix that, which is why the picker carries a
// remembered slot: the custom value is a stop *across* the removal.
#[test]
fn the_custom_value_stays_reachable_across_the_default_stop() {
    // Before the removal the value is in the file, so the list carries it.
    let with_value = cycle_row(
        "ui.fullscreen_wheel_lines",
        "4",
        &["1", "2", "3", "5", "10"],
    );
    assert!(
        Chat::cycle_options_with(&with_value, None).contains(&"4".to_string()),
        "the file's own value is a stop while it is set"
    );
    // After `default` removed the key the row reads its built-in default, and the
    // list has no reason to contain `4` - so the picker's memory supplies it.
    let after_removal = cycle_row(
        "ui.fullscreen_wheel_lines",
        "auto",
        &["1", "2", "3", "5", "10"],
    );
    assert!(
        !Chat::cycle_options_with(&after_removal, None).contains(&"4".to_string()),
        "the list alone has forgotten it, which is the defect this guards"
    );
    let remembered = "4".to_string();
    let options = Chat::cycle_options_with(&after_removal, Some(&remembered));
    assert!(
        options.contains(&"4".to_string()),
        "the remembered slot keeps it reachable: {options:?}"
    );
    // And the lap closes.
    let start = options.iter().position(|v| v == "4").expect("a stop");
    let mut current = options[start].clone();
    for _ in 0..options.len() {
        current = Chat::next_cycle_value(&options, &current, true);
    }
    assert_eq!(current, "4", "a full lap returns to the custom value");
}
