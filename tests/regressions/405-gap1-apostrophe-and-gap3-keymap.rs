//! NFR-24 guard for gh #405 gaps 1 and 3.
//!
//! Two findings, two guards, and both for the same reason: the defect they close
//! is **class-shaped**, so a row that pins one example would pass while the class
//! came back.
//!
//! # Gap 1 - the apostrophe is a code point, not a spelling
//!
//! The French screenshot stem shipped with ASCII `'` while macOS's Finders write
//! `U+2019`, and **NFC normalisation does not convert one into the other** - they
//! are distinct code points, both already normalised. So the shape test failed for a
//! real filename, and the resolver's miss list is generated from the same stem list,
//! which means a naive "fix" at lookup time would print a filename nobody has.
//!
//! # Gap 3 - the Windows default table must not leak into Unix
//!
//! LCA has one static keymap. The remap adds a *family* and a delta, and the failure
//! this guard exists for is the frontier between them: a delta entry applied to the
//! Unix family would move a macOS or Linux binding, which is exactly what the issue
//! forbids. The behavioural rows live beside the code
//! (`engine::keybindings::tests::the_unix_family_defaults_are_the_static_table_unchanged`);
//! this file pins the *structure* those rows depend on, so a future refactor that
//! turns the delta into a second full table reddens here rather than silently
//! duplicating thirty descriptions.
//!
//! Verifies: gh #405 gaps 1 and 3, NFR-24.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use crate::{code_lines, module_text};

/// The resolver's source.
fn paths_source() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/lca-tools/src/paths.rs"),
    )
    .expect("read crates/lca-tools/src/paths.rs")
}

// Verifies: gh #405 gap 1 - **both apostrophe spellings are in the stem list, as
// separate entries.** A substitution (`replace('’', "'")`) would be one entry and one
// rewritten filename; the whole point is that the two code points are different
// names, so both appear.
#[test]
fn the_french_stem_carries_both_apostrophes_as_separate_entries() {
    let source = paths_source();
    assert!(
        source.contains("\"Capture d'écran\""),
        "the ASCII spelling stays (it is what older macOS wrote)"
    );
    assert!(
        source.contains("\"Capture d\\u{2019}écran\""),
        "and the typographic one is a second candidate, never a rewrite: NFC does not \
         turn U+2019 into U+0027"
    );
    // The mapping function may only *generate* candidates: if it also rewrote what
    // the user typed, the miss list would name a file that is not on disk - the one
    // thing `ScreenshotMiss` refuses to do.
    let mapper = source
        .find("fn other_apostrophe")
        .expect("the candidate-side mapping exists");
    let mapper_body = &source[mapper..];
    let end = mapper_body.find("\n}\n").unwrap_or(mapper_body.len());
    assert!(
        mapper_body[..end].contains("stem.replace"),
        "the mapping is a candidate generator over a stem"
    );
    assert!(
        !source.contains("let typed = typed.replace"),
        "nothing rewrites the typed path itself"
    );
}

// Verifies: gh #405 gap 1 - **the candidate list does not repeat a name.** Both
// stems and the per-stem mapping can produce the same filename; a miss list with a
// duplicate reads as a caller bug, and the user sees the resolver's own output.
#[test]
fn the_candidate_generator_de_duplicates() {
    let source = paths_source();
    let generator = source
        .find("pub fn screenshot_candidates")
        .expect("the generator exists");
    let body = &source[generator..];
    let end = body.find("\n}\n").unwrap_or(body.len());
    assert!(
        body[..end].contains("if !names.contains"),
        "the generator checks before pushing, so the set stays a set: {}",
        &body[..end.min(400)]
    );
}

// Verifies: gh #405 gap 3 - **the predicate is pi's, over environment facts.**
// `win32 || (linux && (WSL_DISTRO_NAME || WSL_INTEROP))`, never `WT_SESSION`: a
// terminal is not a keymap, and pi's own test pins that negative. A `cfg!`-only
// implementation cannot satisfy this row because the WSL half has to read the
// environment - which is also why the row can exist on a Linux runner at all.
#[test]
fn the_host_family_predicate_reads_pis_variables_and_not_wt_session() {
    let source = module_text("crates/lca-tui/src/engine/keybindings.rs");
    let predicate = source
        .find("pub fn from_parts")
        .expect("the predicate exists as a pure function");
    let body = &source[predicate..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("WSL_DISTRO_NAME") && body.contains("WSL_INTEROP"),
        "both WSL signals are consulted: {body}"
    );
    assert!(
        !body.contains("WT_SESSION"),
        "WT_SESSION is deliberately excluded - Windows Terminal is a terminal, and \
         pi's own test asserts it is not a keymap: {body}"
    );
    assert!(
        body.contains("\"windows\"") && body.contains("\"linux\""),
        "the platform arrives as a parameter, so all four cases are drivable here: {body}"
    );
}

// Verifies: gh #405 gap 3 - **the remap is a delta, and every entry names a real
// action.** Two halves, both load-bearing:
//
// * the delta carries key lists only, no descriptions - a second description column
//   would be a second table nobody diffs;
// * **each remapped action actually resolves to a different list under the Windows
//   family than under Unix.** That is what makes a typo in the delta's action name a
//   failure rather than a silent no-op: a name the table does not declare resolves to
//   an empty list in *both* families, so "differs" would be false.
//
// The check is behavioural, through the public constructor, because a string scan
// cannot tell a declared action from a mistyped one (measured: the first version of
// this row passed with `app.message.dequeu` in the delta).
#[test]
fn the_windows_remap_is_a_delta_over_the_one_table() {
    use lca_tui::engine::keybindings::{HostFamily, KeybindingsManager};

    let source = module_text("crates/lca-tui/src/engine/keybindings.rs");
    let delta = source
        .find("fn windows_or_wsl_defaults")
        .expect("the delta exists");
    let body = &source[delta..];
    let end = body
        .find("const fn is_native_windows")
        .unwrap_or(body.len());
    let delta = &body[..end];
    assert!(
        !delta.contains("description"),
        "entries carry key lists only; a second description column would be a second \
         table that nobody diffs"
    );

    let unix = KeybindingsManager::with_user_bindings_for(Default::default(), HostFamily::Unix);
    let windows =
        KeybindingsManager::with_user_bindings_for(Default::default(), HostFamily::WindowsOrWsl);
    // **Every action the delta names is one the registry declares.** This is the
    // half that catches a typo (measured: `app.message.dequeu` passed the first
    // version of this row, because a scan of the source cannot tell a declared
    // action from a mistyped one). The list comes from the registry, not the source.
    let declared: Vec<&str> = lca_tui::engine::keybindings::tui_keybindings()
        .iter()
        .map(|(name, _)| *name)
        .collect();
    let mut named: Vec<&str> = Vec::new();
    for line in delta.lines() {
        let Some(start) = line.find("(\"") else {
            continue;
        };
        let rest = &line[start + 2..];
        let Some(end) = rest.find('"') else { continue };
        let action = &rest[..end];
        if !action.contains('.') {
            continue;
        }
        assert!(
            declared.contains(&action),
            "{action} is remapped but the registry never declares it - a typo here \
             would silently do nothing"
        );
        named.push(action);
    }
    // **And the remap really moves them**, except the two entries that are
    // conditional *inside* the family: `HostFamily::WindowsOrWsl` on a non-Windows
    // target resolves to the WSL table, where suspend keeps `ctrl+z` and undo is
    // `alt+z`. A row demanding a difference for those would be asserting the platform
    // it happens to run on; they are pinned per target by
    // `the_windows_family_remaps_pis_keys_and_keeps_the_rest`.
    let conditional = ["app.suspend", "tui.editor.undo"];
    let mut moved = 0;
    for action in &named {
        if conditional.contains(action) {
            continue;
        }
        assert_ne!(
            windows.keys(*action),
            unix.keys(*action),
            "{action} is in the delta but resolves identically in both families"
        );
        moved += 1;
    }
    assert!(
        moved >= 6,
        "the delta really moves the keys it names ({moved} of {named:?})"
    );

    // **Both consts, from the source.** The behavioural half above compares the
    // *reachable* table (on this Linux box that is the WSL const), so a no-op entry
    // inside the native-Windows const would be invisible to it - measured: setting
    // `app.search` to the Unix `ctrl+r` in the Windows const alone left every row
    // green. This reads both consts' entries and compares each key list against the
    // Unix default, so neither arm can be a no-op unseen.
    let unix_defaults = |action: &str| -> Vec<String> {
        lca_tui::engine::keybindings::tui_keybindings()
            .iter()
            .find(|(name, _)| *name == action)
            .map(|(_, definition)| {
                definition
                    .default_keys
                    .iter()
                    .map(|key| (*key).to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut arms_checked = 0;
    for line in delta.lines() {
        let Some(start) = line.find("(\"") else {
            continue;
        };
        let rest = &line[start + 2..];
        let Some(end) = rest.find('"') else { continue };
        let action = &rest[..end];
        if !action.contains('.') {
            continue;
        }
        let Some(keys_start) = line.find("&[") else {
            continue;
        };
        let Some(keys_end) = line[keys_start..].find(']') else {
            continue;
        };
        let list = &line[keys_start + 2..keys_start + keys_end];
        let written: Vec<String> = list
            .split(',')
            .filter_map(|entry| {
                let trimmed = entry.trim();
                let body = trimmed.strip_prefix('"')?.strip_suffix('"')?;
                Some(body.to_string())
            })
            .collect();
        if conditional.contains(&action) {
            continue;
        }
        assert_ne!(
            written,
            unix_defaults(action),
            "{action} is written into the delta with its Unix default, so it is a \
             no-op: {written:?}"
        );
        arms_checked += 1;
    }
    assert!(
        arms_checked >= 12,
        "both delta consts were walked ({arms_checked} entries)"
    );
}

// Verifies: gh #405 gap 3 - **pi's paste key is deliberately not transcribed.**
// `app.clipboard.pasteImage` (`alt+v` on Windows) has no LCA equivalent: a paste is
// `/paste` and the right-button handler, so transcribing it would introduce a
// brand-new binding inside a keymap fix. Pinned so that if the action is ever added,
// this row fails and the Windows delta gains its entry in the same change.
#[test]
fn the_paste_key_lca_does_not_have_is_not_transcribed() {
    let source = module_text("crates/lca-tui/src/engine/keybindings.rs");
    // Comments are exempt: the delta's doc comment names the action **on purpose**,
    // so a reader learns why it is absent. What must not exist is a table entry.
    let code = code_lines(&source);
    assert!(
        !code.contains("app.clipboard.pasteImage"),
        "LCA binds no paste key; adding one is a feature, not part of a remap"
    );
    assert!(
        source.contains("Attaching the paste default"),
        "the reason stays written down beside the delta, so the next reader does not \
         add it as a parity fix"
    );
}
