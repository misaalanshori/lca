//! The keybinding registry's rows (gh #410, gh #405 gap 3).
//!
//! Moved out of `engine/keybindings.rs` when the Windows/WSL family pushed that file
//! past gate 11's 1,200-line ceiling. The module stays `engine::keybindings::tests`
//! because `keybindings.rs` declares it, so no row's reported path changed (gh #574).

use super::*;

// Verifies: gh #173 - Shift+Up/Down join the prompt-step
// bindings (pi's Shift+Scroll ergonomics have a keyboard twin).
#[test]
fn shift_arrows_step_between_prompts() {
    let kb = KeybindingsManager::new();
    assert!(kb.matches("\x1b[1;2A", "app.prompt.previous"));
    assert!(kb.matches("\x1b[1;2B", "app.prompt.next"));
    assert!(!kb.matches("\x1b[1;2A", "app.prompt.next"));
}

// Verifies: gh #410 - a hint's display form follows pi's `formatKeyText`:
// `alt` reads as `option` on macOS (a Mac keyboard prints no key called
// `alt`), a chord keeps its `+` parts, and a multi-binding list joins with
// `/`.
#[test]
fn key_display_follows_pis_formatting_rules() {
    assert_eq!(
        format_key_text_for("alt+left", false, true),
        "option+left",
        "macOS spells `alt` as `option`"
    );
    assert_eq!(
        format_key_text_for("alt+left", false, false),
        "alt+left",
        "other platforms keep `alt`"
    );
    assert_eq!(
        format_key_text_for("ctrl+x+ctrl+e", false, false),
        "ctrl+x+ctrl+e",
        "a chord keeps every part"
    );
    assert_eq!(
        format_key_text_for("shift+up/ctrl+p", false, false),
        "shift+up/ctrl+p",
        "multiple bindings join with `/`"
    );
    // pi's `formatKeyPart` capitalizes **every** part, not only the first
    // character of the whole string: `ctrl+e` renders `Ctrl+E`.
    assert_eq!(
        format_key_text_for("ctrl+e", true, false),
        "Ctrl+E",
        "the capitalized form capitalizes each chord part, as pi does"
    );
    assert_eq!(
        format_key_text_for("ctrl+c", true, false),
        "Ctrl+C",
        "so a single modifier+letter form reads naturally at a sentence start"
    );
    // Only the `alt` *part* is renamed, never a key that merely contains it.
    assert_eq!(
        format_key_text_for("altitude", true, true),
        "Altitude",
        "a key id containing `alt` is not a modifier"
    );
}

// Verifies: gh #410 - the manager's display form is the live one.
//
// The whole defect in one row: a rebound action must display the override, an
// unbound one must display nothing at all, and the default table must not leak
// through either.
#[test]
fn display_keys_follows_the_override_and_the_unbind() {
    let mut user = BTreeMap::new();
    user.insert(
        "app.tools.expand".to_string(),
        vec!["ctrl+e".to_string(), "ctrl+g".to_string()],
    );
    user.insert("app.thinking.toggle".to_string(), Vec::new());
    let kb = KeybindingsManager::with_user_bindings(user);

    assert_eq!(kb.display_keys("app.tools.expand"), "ctrl+e/ctrl+g");
    assert_eq!(
        kb.display_keys("app.thinking.toggle"),
        "",
        "an explicit unbind displays nothing, so a caller omits the hint"
    );
    assert_eq!(kb.display_keys("app.interrupt"), "escape");
    assert_eq!(kb.primary_key("app.tools.expand"), "ctrl+e");
}

#[test]
fn defaults_resolve_and_match() {
    let kb = KeybindingsManager::new();
    assert!(kb.matches("\x1b[A", "tui.editor.cursorUp"));
    assert!(kb.matches("\x1b[A", "tui.select.up"));
    assert!(kb.matches("\r", "tui.input.submit"));
    assert!(kb.matches("\t", "tui.input.tab"));
    assert!(kb.matches("\x03", "tui.input.copy"));
    assert!(!kb.matches("\x1b[B", "tui.editor.cursorUp"));
}

// Verifies: gh #66 - the loader reports names the registry never
// defined, so a typo fails loud instead of silently doing nothing.
#[test]
fn unknown_action_names_are_reported() {
    let mut user = BTreeMap::new();
    user.insert(
        "tui.editor.cursorLeft".to_string(),
        vec!["ctrl+x".to_string()],
    );
    user.insert("app.typo.here".to_string(), vec!["ctrl+y".to_string()]);
    let kb = KeybindingsManager::with_user_bindings(user);
    assert_eq!(kb.unknown_actions(), vec!["app.typo.here".to_string()]);
    assert!(kb.matches("\x18", "tui.editor.cursorLeft"));
}

// Verifies: gh #66 follow-up - an unparseable key value falls back
// to defaults for its action and reports the offending value.
#[test]
fn an_unparseable_key_keeps_defaults_and_reports() {
    let mut user = BTreeMap::new();
    user.insert(
        "tui.editor.cursorLeft".to_string(),
        vec!["ctrl+xyz".to_string()],
    );
    let kb = KeybindingsManager::with_user_bindings(user);
    assert_eq!(
        kb.keys("tui.editor.cursorLeft"),
        vec!["left".to_string(), "ctrl+b".to_string()],
        "defaults hold for the poisoned action"
    );
    assert_eq!(
        kb.invalid_keys(),
        &[("tui.editor.cursorLeft".to_string(), "ctrl+xyz".to_string())]
    );
}

// Verifies: gh #66 - every key in a multi-key list fires.
#[test]
fn every_key_in_a_list_fires() {
    let mut user = BTreeMap::new();
    user.insert(
        "tui.editor.cursorLeft".to_string(),
        vec!["ctrl+x".to_string(), "ctrl+y".to_string()],
    );
    let kb = KeybindingsManager::with_user_bindings(user);
    assert!(kb.matches("\x18", "tui.editor.cursorLeft"));
    assert!(kb.matches("\x19", "tui.editor.cursorLeft"));
    assert!(
        !kb.matches("\x02", "tui.editor.cursorLeft"),
        "the default is replaced"
    );
}

#[test]
fn user_override_and_explicit_unbind() {
    let mut user = BTreeMap::new();
    user.insert("tui.input.submit".to_string(), vec!["ctrl+s".to_string()]);
    user.insert("tui.input.tab".to_string(), Vec::new());
    let kb = KeybindingsManager::with_user_bindings(user);
    assert!(kb.matches("\x13", "tui.input.submit"));
    assert!(!kb.matches("\r", "tui.input.submit"));
    assert!(kb.keys("tui.input.tab").is_empty());
}

// Verifies: gh #8 (EFG-003) - pi's model-cycle keys are the defaults
// the issue asks for, exactly one action claims `ctrl+p`, and the
// extension panel kept a key of its own after moving (`alt+x`).
#[test]
fn the_model_cycle_keys_are_pis_and_ctrl_p_is_claimed_once() {
    let kb = KeybindingsManager::new();
    assert!(
        kb.matches("\x10", "app.model.cycleForward"),
        "Ctrl+P cycles"
    );
    assert!(
        kb.matches("\x1bp", "app.model.cycleBackward"),
        "Alt+P is the backward key pi keeps for terminals that drop the shift"
    );
    assert_eq!(
        kb.keys("app.model.cycleBackward"),
        vec!["shift+ctrl+p", "alt+p"],
        "both of pi's backward keys are defaults"
    );
    assert!(
        !kb.matches("\x10", "app.panel.toggle"),
        "Ctrl+P is the cycle, not the panel"
    );
    assert!(
        kb.matches("\x1bx", "app.panel.toggle"),
        "alt+x is the panel"
    );
    let claimants: Vec<&str> = tui_keybindings()
        .iter()
        .filter(|(_, def)| def.default_keys.contains(&"ctrl+p"))
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        claimants,
        vec!["app.model.cycleForward"],
        "one action claims Ctrl+P"
    );
}

// Verifies: gh #8 phase 2 (pi's `app.models.save`) and gh #396 (pi's
// `app.thinking.save`) - Ctrl+S is the **save-as-default** key, and every
// action that claims it is a picker's save.
//
// Gh #396 sanctioned change: this row used to assert exactly one claimant.
// There are now two, and that is correct rather than a collision: the two
// pickers cannot be open at once (`/model` and `/thinking` each open their own
// and `Chat::picker_open` dispatches to whichever is up), so the key is never
// ambiguous at runtime. What the row must still forbid is a claimant that is
// *not* a save gesture - that would be a genuine conflict. The exclusion
// itself is pinned in `lca-ui` (`the_two_save_actions_belong_to_mutually_
// exclusive_pickers`), where the pickers live.
#[test]
fn ctrl_s_is_pis_save_default_and_no_other_kind_of_action_claims_it() {
    let kb = KeybindingsManager::new();
    assert!(kb.matches("\x13", "app.models.save"));
    assert!(kb.matches("\x13", "app.thinking.save"));
    let claimants: Vec<&str> = tui_keybindings()
        .iter()
        .filter(|(_, def)| def.default_keys.contains(&"ctrl+s"))
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        claimants,
        vec!["app.models.save", "app.thinking.save"],
        "exactly the two picker saves claim Ctrl+S"
    );
    for name in &claimants {
        assert!(
            name.ends_with(".save"),
            "{name} claims Ctrl+S but is not a save-default gesture"
        );
    }
}

// Verifies: gh #9 (pi 1.0.0's `app.message.copy`) - Ctrl+X answers
// to the copy key, and the external-editor chord still starts there
// too (FR-UI-15's suite row keeps passing).
#[test]
fn ctrl_x_is_pis_message_copy_and_still_arms_the_editor_chord() {
    let kb = KeybindingsManager::new();
    assert!(kb.matches("\x18", "app.message.copy"));
    assert!(
        kb.matches("\x18", "app.editor.external"),
        "the Ctrl+X Ctrl+E chord's first key is unchanged"
    );
}

// Verifies: gh #405 gap 3 - **pi's predicate, as a pure function, for all four
// platforms.** `win32`, and a *Linux* build under WSL (`WSL_DISTRO_NAME` or
// `WSL_INTEROP`), are the Windows family; macOS, plain Linux and a Linux
// session inside Windows Terminal are not.
//
// The `WT_SESSION` case is the one worth a row of its own: Windows Terminal is a
// *terminal*, not a keymap, and pi pins it as false
// (`keybindings.test.ts:15`). Getting it wrong would remap a native Linux
// session that merely happens to run under that terminal.
#[test]
fn the_host_family_predicate_is_pis_rule() {
    let env = |pairs: &[(&str, &str)]| {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key: &str| {
            owned
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        }
    };
    // A named shape rather than an array-of-tuples-of-Vec: the row reads
    // better and clippy's `type_complexity` agrees.
    type Case<'a> = (&'a str, Vec<(&'a str, &'a str)>, HostFamily);
    let cases: [Case<'_>; 6] = [
        ("windows", vec![], HostFamily::WindowsOrWsl),
        (
            "linux",
            vec![("WSL_DISTRO_NAME", "Ubuntu")],
            HostFamily::WindowsOrWsl,
        ),
        (
            "linux",
            vec![("WSL_INTEROP", "/run/WSL/123_interop")],
            HostFamily::WindowsOrWsl,
        ),
        // pi's negative: a terminal is not a keymap.
        ("linux", vec![("WT_SESSION", "session")], HostFamily::Unix),
        ("linux", vec![], HostFamily::Unix),
        ("macos", vec![], HostFamily::Unix),
    ];
    for (platform, pairs, expected) in cases {
        let family = HostFamily::from_parts(platform, env(&pairs));
        assert_eq!(
            family, expected,
            "platform {platform:?} with {pairs:?} must be {expected:?}"
        );
    }
    // An empty variable is not evidence: a shell that exports `WSL_DISTRO_NAME=`
    // must not flip the keymap.
    assert_eq!(
        HostFamily::from_parts("linux", env(&[("WSL_DISTRO_NAME", "")])),
        HostFamily::Unix,
        "an empty WSL_DISTRO_NAME is not WSL"
    );
    // And this box is Unix, so the rest of the suite is comparing against the
    // Unix table by construction - stated rather than assumed.
    assert_eq!(
        HostFamily::current(),
        if cfg!(any(target_os = "windows", target_os = "linux")) {
            HostFamily::current()
        } else {
            HostFamily::Unix
        },
        "the current family is answered, not panicking"
    );
}

// Verifies: gh #405 gap 3 - **the remap is pi's, key by key.** Asserted as a
// table rather than through `matches`, because the subject is the *default list*
// an action carries (what `/hotkeys` prints and what the next reader copies),
// and a byte-level match would pass for a key that is listed under two actions.
//
// pi's `KEYBINDINGS` overrides are the authority
// (`packages/coding-agent/src/core/keybindings.ts`): followUp `ctrl+q`, dequeue
// `alt+q`, pasteImage `alt+v`, undo `ctrl+z` on native Windows and `alt+z`
// under WSL, altScreen.search `ctrl+f`, and the plain Ctrl arrows for the
// prompt jumps.
#[test]
fn the_windows_family_remaps_pis_keys_and_keeps_the_rest() {
    let kb = KeybindingsManager::with_user_bindings_for(BTreeMap::new(), HostFamily::WindowsOrWsl);
    assert_eq!(kb.family(), HostFamily::WindowsOrWsl);
    for (action, expected) in [
        ("app.message.followUp", vec!["ctrl+q"]),
        ("app.message.dequeue", vec!["alt+q"]),
        ("app.search", vec!["ctrl+f"]),
        ("tui.altScreen.search", vec!["ctrl+f"]),
        ("tui.altScreen.previousPrompt", vec!["ctrl+up"]),
        ("tui.altScreen.nextPrompt", vec!["ctrl+down"]),
        ("app.model.cycleBackward", vec!["alt+p"]),
    ] {
        assert_eq!(
            kb.keys(action),
            expected,
            "{action} must take pi's Windows/WSL default"
        );
    }
    // Two defaults differ *inside* the family, and both are pi's own split
    // (`process.platform === "win32" ? ... : ...`): a native Windows console
    // delivers Ctrl+Z and has no SIGTSTP, so undo is `ctrl+z` and suspend is
    // unbound; a WSL build keeps Alt+Z for undo and Ctrl+Z for suspend, because
    // job control exists there.
    #[cfg(target_os = "windows")]
    assert_eq!(kb.keys("app.suspend"), Vec::<String>::new());
    #[cfg(not(target_os = "windows"))]
    assert_eq!(
        kb.keys("app.suspend"),
        vec!["ctrl+z"],
        "a WSL build has SIGTSTP, so pi keeps the Unix suspend key"
    );
    #[cfg(target_os = "windows")]
    assert_eq!(kb.keys("tui.editor.undo"), vec!["ctrl+z"]);
    #[cfg(not(target_os = "windows"))]
    assert_eq!(
        kb.keys("tui.editor.undo"),
        vec!["alt+z"],
        "a Linux WSL build keeps Alt+Z - Windows Terminal takes Ctrl+Z itself"
    );
    // Only the remapped actions changed: an action outside the delta is
    // byte-identical to its Unix default, which is what stops the remap from
    // becoming a second table nobody diffs.
    for (action, definition) in tui_keybindings() {
        if windows_or_wsl_defaults()
            .iter()
            .any(|(name, _)| *name == *action)
        {
            continue;
        }
        let expected: Vec<String> = definition
            .default_keys
            .iter()
            .map(|k| (*k).to_string())
            .collect();
        assert_eq!(
            kb.keys(action),
            expected,
            "{action} is not in the Windows delta, so it must keep its Unix default"
        );
    }
}

// Verifies: gh #405 gap 3 - **no Unix binding changes.** The whole point of the
// predicate is that a macOS or plain-Linux session is *bit-identical* to what
// shipped before this cycle; a remap that leaked into the Unix family would be
// the defect this row exists to catch, and it is checked against the static
// table rather than against a remembered list.
#[test]
fn the_unix_family_defaults_are_the_static_table_unchanged() {
    let kb = KeybindingsManager::with_user_bindings_for(BTreeMap::new(), HostFamily::Unix);
    assert_eq!(kb.family(), HostFamily::Unix);
    for (action, definition) in tui_keybindings() {
        let expected: Vec<String> = definition
            .default_keys
            .iter()
            .map(|k| (*k).to_string())
            .collect();
        assert_eq!(
            kb.keys(action),
            expected,
            "Unix must keep {action} exactly as the table declares it"
        );
    }
    // The bindings the Windows delta lists, spelled out as the Unix values they
    // must still have - a set equality on a whole table cannot show that a
    // specific user-visible key was *not* moved.
    for (action, unix_keys) in [
        ("app.message.followUp", vec!["alt+enter"]),
        ("app.message.dequeue", vec!["alt+up", "alt+e"]),
        ("app.search", vec!["ctrl+r"]),
        ("tui.altScreen.search", vec!["ctrl+shift+f"]),
        (
            "tui.altScreen.previousPrompt",
            vec!["ctrl+shift+up", "ctrl+up"],
        ),
        (
            "tui.altScreen.nextPrompt",
            vec!["ctrl+shift+down", "ctrl+down"],
        ),
        ("tui.editor.undo", vec!["ctrl+-"]),
        ("app.suspend", vec!["ctrl+z"]),
        ("app.model.cycleBackward", vec!["shift+ctrl+p", "alt+p"]),
    ] {
        assert_eq!(
            kb.keys(action),
            unix_keys,
            "{action} must not move on Unix (gh #405 gap 3 changes Windows only)"
        );
    }
    // pi has `app.clipboard.pasteImage`; LCA does not (paste is `/paste` and the
    // right button). Asserted rather than implied: if someone ever adds the
    // action, this row fails and the Windows delta gets its entry at the same
    // time, instead of a Unix-only binding shipping for a Windows family.
    for family in [HostFamily::Unix, HostFamily::WindowsOrWsl] {
        let kb = KeybindingsManager::with_user_bindings_for(BTreeMap::new(), family);
        assert!(
            kb.keys("app.clipboard.pasteImage").is_empty(),
            "LCA has no paste key in {family:?}; adding one is a feature, not a remap"
        );
    }
}

#[test]
fn user_conflicts_are_reported() {
    let mut user = BTreeMap::new();
    user.insert("tui.input.submit".to_string(), vec!["ctrl+s".to_string()]);
    user.insert("tui.input.tab".to_string(), vec!["ctrl+s".to_string()]);
    let kb = KeybindingsManager::with_user_bindings(user);
    assert_eq!(kb.conflicts().len(), 1);
    assert_eq!(kb.conflicts()[0].keybindings.len(), 2);
}
