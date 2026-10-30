//! Rows for the key matcher (`keys.rs`).
//!
//! Split out when `keys.rs` crossed the 1,200-line ceiling after gh #377 added the
//! shifted-symbol rows.

use super::*;

/// Match with an explicit Kitty state (gh #323 defect 2).
///
/// **This replaces a `with_kitty` helper that mutated the process-global flag** -
/// the sanctioned test change the brief names. Every row below that used it now
/// passes its state as a parameter, so the rows no longer race each other under
/// `nextest`'s parallelism (the old helper set a static that a concurrently running
/// row could observe, and restore it out from under that row).
fn kitty(active: bool) -> Option<bool> {
    Some(active)
}

// Verifies: gh #66 follow-up - the key vocabulary accepts every
// shape `matches_key` handles and rejects anything else, so a
// `ctrl+xyz` typo reports loud instead of never matching.
#[test]
fn the_key_vocabulary_accepts_named_and_single_keys_only() {
    for valid in [
        "up",
        "down",
        "left",
        "right",
        "pageUp",
        "pageDown",
        "home",
        "end",
        "escape",
        "esc",
        "enter",
        "return",
        "tab",
        "space",
        "backspace",
        "delete",
        "insert",
        "clear",
        "f1",
        "f12",
        "a",
        "z",
        "5",
        ";",
        "ctrl+x",
        "ctrl+shift+n",
        "alt+left",
        "super+k",
    ] {
        assert!(is_valid_key_id(valid), "{valid} is a key");
    }
    for invalid in ["", "ctrl+xyz", "super-duper", "shift", "f13", "enter+"] {
        assert!(!is_valid_key_id(invalid), "{invalid} is not a key");
    }
}

// Verifies: I.1 key dialect tables (pi keys.ts golden cases).
#[test]
fn legacy_sequences_match_their_key_ids() {
    assert!(matches_key("\x1b[A", "up"));
    assert!(matches_key("\x1bOA", "up"));
    assert!(matches_key("\x1b[B", "down"));
    assert!(matches_key("\x1b[5~", "pageUp"));
    assert!(matches_key("\x1b[6~", "pageDown"));
    assert!(matches_key("\x1b[H", "home"));
    assert!(matches_key("\x1b[F", "end"));
    assert!(matches_key("\x1b[3~", "delete"));
    assert!(matches_key("\x1b[2~", "insert"));
    assert!(matches_key("\x1bOP", "f1"));
    assert!(matches_key("\x1b[24~", "f12"));
}

#[test]
fn ctrl_letters_use_control_codes() {
    assert!(matches_key("\x03", "ctrl+c"));
    assert!(matches_key("\x1b[99;5u", "ctrl+c")); // Kitty CSI-u
    assert!(matches_key("\x1b[27;5;99~", "ctrl+c")); // modifyOtherKeys
    assert!(!matches_key("\x03", "ctrl+d"));
}

#[test]
fn shift_tab_and_shift_enter_dialects() {
    assert!(matches_key("\x1b[Z", "shift+tab"));
    assert!(matches_key("\x1b[9;2u", "shift+tab"));
    assert!(matches_key("\x1b[13;2u", "shift+enter"));
    assert!(matches_key_with_kitty("\x1b\r", "shift+enter", kitty(true)));
    assert!(matches_key_with_kitty("\n", "shift+enter", kitty(true)));
    assert!(!matches_key_with_kitty("\x1b\r", "alt+enter", kitty(true)));
    assert!(matches_key_with_kitty("\x1b\r", "alt+enter", kitty(false)));
}

#[test]
fn base_layout_key_policy_prefers_recognizable_codepoints() {
    // Latin recognisable -> codepoint authoritative, base layout ignored.
    assert!(!matches_key("\x1b[118:118:107;5u", "ctrl+k")); // reports v with base k
    // Cyrillic -> base layout key rescues the match.
    assert!(matches_key("\x1b[1089:0:99;5u", "ctrl+c"));
}

#[test]
fn keypad_normalizes_to_ascii_equivalents() {
    assert!(matches_key("\x1b[57404u", "5"));
    assert!(matches_key("\x1b[57414u", "enter"));
}

#[test]
fn escape_alone_and_alt_prefixed_printables() {
    assert!(matches_key("\x1b", "escape"));
    assert!(matches_key("\x1b[27u", "escape"));
    assert!(matches_key("\x1b[27;1u", "escape"));
    assert!(matches_key("\x1b[27;1;27~", "escape"));
    assert!(matches_key_with_kitty("\x1bx", "alt+x", kitty(false)));
    assert!(matches_key_with_kitty(
        "\x1b\x03",
        "ctrl+alt+c",
        kitty(false)
    ));
}

#[test]
fn arrow_modifier_dialects() {
    assert!(matches_key("\x1b[1;3D", "alt+left"));
    assert!(matches_key("\x1b[1;5C", "ctrl+right"));
    assert!(matches_key("\x1bb", "alt+left"));
    assert!(matches_key("\x1bf", "alt+right"));
}

#[test]
fn release_and_repeat_sniffing_vetoes_paste() {
    assert!(is_key_release("\x1b[99;5:3u"));
    assert!(is_key_repeat("\x1b[99;5:2u"));
    assert!(!is_key_release("\x1b[200~90:62:3F:A5\x1b[201~"));
    assert!(!is_key_repeat("\x1b[200~x:2F:y\x1b[201~"));
}

#[test]
fn parse_key_round_trips_common_inputs() {
    assert_eq!(parse_key("\x1b[A").as_deref(), Some("up"));
    assert_eq!(parse_key("\r").as_deref(), Some("enter"));
    assert_eq!(parse_key("\x03").as_deref(), Some("ctrl+c"));
    assert_eq!(parse_key("\x1b[Z").as_deref(), Some("shift+tab"));
    assert_eq!(parse_key("a").as_deref(), Some("a"));
    assert_eq!(parse_key("\x1b[13;2u").as_deref(), Some("shift+enter"));
}

#[test]
fn decode_printable_accepts_plain_and_shifted_only() {
    assert_eq!(decode_printable_key("\x1b[97u").as_deref(), Some("a"));
    assert_eq!(decode_printable_key("\x1b[97:65;2u").as_deref(), Some("A"));
    assert_eq!(decode_printable_key("\x1b[27;2;97~").as_deref(), Some("a"));
    assert!(decode_printable_key("\x1b[97;5u").is_none()); // ctrl rejected
    assert!(decode_printable_key("\x1b[97;3u").is_none()); // alt rejected
}

// ==== gh #377: a shifted symbol is matchable by its logical spelling ====

// Kitty flag 4 reports `Ctrl+Shift+/` as codepoint `'/'` (47) plus a separate
// shifted key `'?'` (63) and modifier `ctrl|shift` (6, 1-based).
const KITTY_CTRL_SHIFT_SLASH: &str = "\x1b[47:63;6u";
// xterm `modifyOtherKeys` reports the same event as the produced codepoint.
const MOK_CTRL_SHIFT_SLASH: &str = "\x1b[27;6;63~";

// Verifies: gh #377 AC1 - **the issue's own example**: the Kitty sequence for
// `Ctrl+Shift+/` matches a binding written the natural way, `ctrl+?`.
//
// Before the fix `matches_kitty` compared codepoint 47 against 63, so the binding
// could never fire; the user had to discover the physical `ctrl+shift+/` spelling.
#[test]
fn kitty_ctrl_shift_slash_matches_a_binding_on_ctrl_question() {
    assert!(
        matches_key(KITTY_CTRL_SHIFT_SLASH, "ctrl+?"),
        "the logical spelling must match: ctrl+?"
    );
}

// Verifies: gh #377 AC2 - **one decision, two protocols**: the `modifyOtherKeys`
// encoding of the same event matches the same binding.
#[test]
fn modify_other_keys_ctrl_shift_slash_matches_the_same_binding() {
    assert!(
        matches_key(MOK_CTRL_SHIFT_SLASH, "ctrl+?"),
        "modifyOtherKeys must reach the same logical decision as kitty"
    );
}

// Verifies: gh #377 AC3 - **both spellings work** where the protocol reports the
// physical key. Kitty does: it sends codepoint `'/'` plus shifted `'?'`, so a
// binding on `ctrl+shift+/` matches physically and a binding on `ctrl+?` matches
// logically - the same event, two spellings, both live.
//
// `modifyOtherKeys` is **not** covered here, and that is pi's behaviour rather than
// a gap in this fix: that protocol reports the *produced* codepoint (`'?'`, 63) and
// no physical one, so `ctrl+shift+/` has nothing to compare against. pi's
// `matchesPrintably` has the same shape - its physical branch compares
// `parsed.codepoint` (63) to `expectedKeycode` (47) and misses. Kitty is the
// protocol that carries both, which is why the physical spelling survives there.
#[test]
fn the_physical_spelling_still_matches_where_the_protocol_reports_it() {
    assert!(
        matches_key(KITTY_CTRL_SHIFT_SLASH, "ctrl+shift+/"),
        "kitty carries the physical key, so the physical spelling must not be \
         deprecated"
    );
    assert!(
        matches_key(KITTY_CTRL_SHIFT_SLASH, "ctrl+?"),
        "and the logical spelling matches the same event"
    );
    // A *plain* ctrl+/ is a different event and must not match the shifted binding.
    assert!(
        !matches_key("\x1b[47;5u", "ctrl+?"),
        "ctrl+/ alone is not ctrl+?"
    );
    // `modifyOtherKeys` spells it the produced way only - documented above.
    assert!(
        matches_key(MOK_CTRL_SHIFT_SLASH, "ctrl+?"),
        "which is exactly what this protocol can express"
    );
}

// Verifies: gh #377 rule 3 - the vocabulary is **bounded to symbols**.
//
// A logical candidate built from any shifted character would make `shift+a` match a
// binding on `A` through the logical path while the physical path also matched, and
// the two paths could then disagree about modifiers. Shifted letters have their own
// identity rule; only symbols take the logical route.
#[test]
fn a_shifted_letter_does_not_take_the_logical_route() {
    // The bound is only observable through a binding the matcher will *consider*.
    // A binding on `A` never reaches the logical path at all - `is_valid_key_id`
    // rejects an uppercase letter first - so asserting `!matches_key(seq, "A")`
    // proves nothing about the vocabulary (it defeated my first version of this
    // row, which survived a mutation that removed the bound entirely).
    //
    // The reachable case is `ctrl+A`-shaped: a binding on an uppercase letter with
    // a modifier is legal to *write* (the loader accepts it, since the key is
    // lowercased), and a shifted-letter event carrying ctrl must not satisfy it
    // through the logical path.
    let kitty_ctrl_shift_a = "\x1b[97:65;6u"; // codepoint 'a', shifted 'A', ctrl|shift
    // This one *does* match, physically (codepoint 'a' + ctrl|shift). The property
    // under test is that it matches **that** way and not as `ctrl+a` - which is what
    // an unbounded vocabulary would also let it do, by consuming the Shift.
    assert!(
        matches_key(kitty_ctrl_shift_a, "ctrl+shift+a"),
        "the physical spelling matches"
    );
    assert!(
        !matches_key(kitty_ctrl_shift_a, "ctrl+a"),
        "a shifted letter must not collapse to the unshifted binding - that is what \
         an unbounded symbol vocabulary would allow, and it would let `ctrl+a` fire \
         on a keystroke the user did not type"
    );
    // And the bound in the rule itself: `logical_shifted_symbol` refuses a letter.
    assert_eq!(
        logical_shifted_symbol(65, MOD_SHIFT),
        None,
        "an uppercase letter is not a shifted symbol: {} must not be a candidate",
        SHIFTED_SYMBOL_KEYS
    );
    // A symbol at the same modifier *is* a candidate, so the row above is not
    // passing because the function refuses everything.
    assert_eq!(
        logical_shifted_symbol(63, MOD_SHIFT),
        Some((63, 0)),
        "`?` is, so the vocabulary is what refused `A`"
    );
    // Physical spelling of a shifted letter still works.
    assert!(
        matches_key("\x1b[97:65;2u", "shift+a"),
        "the physical spelling is how a shifted letter is named"
    );
}

// Verifies: gh #377 AC5 - Alt is preserved: only the implicit Shift is consumed.
#[test]
fn alt_survives_the_shift_consumption() {
    // Ctrl+Alt+Shift+/ -> codepoint '/', shifted '?', mod = shift|alt|ctrl = 8 (1-based).
    let seq = "\x1b[47:63;8u";
    assert!(
        matches_key(seq, "ctrl+alt+?"),
        "ctrl+alt+? must match the logical candidate"
    );
    assert!(
        !matches_key(seq, "ctrl+?"),
        "and must NOT match a binding that does not require Alt"
    );
}

// Verifies: gh #377 AC6 - `super` is preserved too: only Shift is consumed, so a
// Super+Shift+/ event matches `super+?` and not `ctrl+?`.
#[test]
fn super_survives_and_is_required() {
    // super|shift = 1 + 8 = 9 (1-based).
    let seq = "\x1b[47:63;10u";
    assert!(matches_key(seq, "super+?"), "super+? must match");
    assert!(
        !matches_key(seq, "?"),
        "a Super-modified event must not match a bare binding"
    );
}

// Verifies: gh #377 AC7 - modifiers are not lost in general: `ctrl+alt+?` requires
// Alt, and `ctrl+?` does not accept an event carrying Alt.
#[test]
fn modifiers_are_never_dropped_by_the_logical_path() {
    for (seq, expected) in [
        ("\x1b[47:63;6u", "ctrl+?"),     // ctrl|shift -> ctrl+?
        ("\x1b[47:63;8u", "ctrl+alt+?"), // ctrl|alt|shift -> ctrl+alt+?
        ("\x1b[47:63;4u", "alt+?"),      // alt|shift -> alt+?
    ] {
        assert!(matches_key(seq, expected), "{seq} must match {expected}");
    }
    // Shift **alone** is consumed entirely, so the candidate has no modifiers left:
    // the logical spelling is the bare symbol `?`, not `shift+?`. (Physically the
    // event's codepoint is `'/'`, so `shift+?` was never expressible.)
    assert!(
        matches_key("\x1b[47:63;2u", "?"),
        "shift+? is spelled `?` once the shift is consumed"
    );
}

// Verifies: gh #377 - `alt+~` (the issue's other example) matches, since `~` is a
// shifted symbol (`shift+`` `).
#[test]
fn alt_tilde_matches_through_the_logical_path() {
    // alt|shift with codepoint '`' (96) shifted to '~' (126).
    let seq = "\x1b[96:126;4u";
    assert!(
        matches_key(seq, "alt+~"),
        "alt+~ is the logical spelling of alt+shift+`"
    );
}

// Verifies: gh #377 - the **decode** path already used the shifted key, and the
// **match** path was the one discarding it. They now agree about which symbol the
// event produces; what differs is which modifiers each accepts, deliberately.
//
// Typing refuses Ctrl/Alt (pi's `decodeKittyPrintable`:
// `if (modifier & (alt | ctrl)) return undefined`) because a Ctrl chord is a
// command, not text. A *binding* must accept it, because that is exactly the case
// this issue is about. Same shifted symbol, two consumers, different modifier
// policy.
#[test]
fn typing_and_binding_agree_on_the_symbol_and_differ_on_modifiers() {
    // Shift alone is typing: the shifted key becomes the character.
    assert_eq!(
        decode_printable_key("\x1b[47:63;2u"),
        Some("?".to_string()),
        "typing produces the shifted symbol"
    );
    // With Ctrl it is a command, not text - refused by the typing path...
    assert_eq!(
        decode_printable_key(KITTY_CTRL_SHIFT_SLASH),
        None,
        "a Ctrl chord is not typed text"
    );
    // ...but the binding path must name the same symbol.
    assert!(
        matches_key(KITTY_CTRL_SHIFT_SLASH, "ctrl+?"),
        "and a binding matches it"
    );
}

// Verifies: gh #377 - a malformed CSI-u with extra digits declines rather than
// panicking or matching something arbitrary.
#[test]
fn a_malformed_shifted_sequence_declines() {
    for bad in [
        "\x1b[47:63;6:2:9u", // too many colon fields
        "\x1b[47:;6u",       // empty shifted field followed by a modifier
        "\x1b[;6u",          // no codepoint
        "\x1b[47:63u",       // no modifier field: valid (mod defaults to none)
    ] {
        // The first three must not claim a ctrl+? binding.
        if bad != "\x1b[47:63u" {
            assert!(!matches_key(bad, "ctrl+?"), "{bad:?} must not match ctrl+?");
        }
        // None of them may panic, whatever they answer.
        let _ = matches_key(bad, "ctrl+shift+/");
    }
}

// Verifies: gh #323 defect 2 - **two terminals can hold different Kitty states at
// the same instant**, and each match answers from its own.
//
// This is the row a `thread_local` fails, and it is the reason the flag is a
// per-manager cell rather than one. Both managers here live on this thread and are
// consulted in the same breath: `\x1b\r` must mean `shift+enter` for the terminal
// that negotiated Kitty and `alt+enter` for the one that did not. A thread-local
// would hand both the same answer; a process-global would hand both whichever was
// set last.
//
// The failure this prevents is concrete: a second pane (or a second `lca` view in one
// process) that never enabled Kitty would reinterpret every Shift+Enter as Alt+Enter,
// because the first terminal's negotiation won process-wide.
#[test]
fn two_managers_disagree_about_kitty_simultaneously() {
    use crate::engine::keybindings::KeybindingsManager;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let kitty_terminal = Arc::new(AtomicBool::new(true));
    let legacy_terminal = Arc::new(AtomicBool::new(false));
    let mut kitty_manager = KeybindingsManager::new();
    kitty_manager.set_kitty_state(kitty_terminal.clone());
    let mut legacy_manager = KeybindingsManager::new();
    legacy_manager.set_kitty_state(legacy_terminal.clone());

    // Interleaved, not sequential: the point is that they coexist, so a stale global
    // written by the first could not explain the second's answer.
    for round in 0..3 {
        assert!(
            kitty_manager.matches("\x1b\r", "tui.input.newLine"),
            "round {round}: the Kitty terminal reads Shift+Enter"
        );
        assert!(
            !matches_key_with_kitty("\x1b\r", "alt+enter", kitty(true)),
            "round {round}: and the Kitty terminal does not read Alt+Enter"
        );
        assert!(
            matches_key_with_kitty("\x1b\r", "alt+enter", kitty(false)),
            "round {round}: the legacy terminal reads Alt+Enter"
        );
        assert!(
            !legacy_manager.matches("\x1b\r", "tui.input.newLine"),
            "round {round}: and not Shift+Enter"
        );
    }

    // Negotiation is live: the terminal flips its own cell and the manager follows
    // without any notification, which is why the state is a shared `Arc` and not a
    // copied `bool`.
    legacy_terminal.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        legacy_manager.matches("\x1b\r", "tui.input.newLine"),
        "a terminal that negotiates Kitty later is followed"
    );
    assert!(
        kitty_manager.matches("\x1b\r", "tui.input.newLine"),
        "and the other terminal is unaffected"
    );
}

// Verifies: gh #323 - a manager with no terminal attached keeps the old ambient
// behaviour, so the change is additive rather than a new required wiring.
#[test]
fn a_manager_without_a_terminal_uses_the_ambient_state() {
    use crate::engine::keybindings::KeybindingsManager;
    let manager = KeybindingsManager::new();
    assert!(!manager.has_kitty_state(), "nothing attached by default");
    let prev = is_kitty_protocol_active();
    // Ambient on: the unattached manager follows it, as it always did.
    set_kitty_protocol_active(true);
    assert!(manager.matches("\x1b\r", "tui.input.newLine"));
    set_kitty_protocol_active(prev);
}
