//! GitHub #387 (gaps 1 and 2): the `system` palette's panels are visible, and
//! its scheme detection is not inverted.
//!
//! # Gap 1 — panels were invisible
//!
//! `Palette::system` set `UserMessageBg` and `CustomMessageBg` **equal to** the
//! probed terminal background. A band the same colour as the page it sits on is
//! not a band: a user message and a tool card were indistinguishable from the
//! transcript, which is the one thing a message card exists to prevent. The tool
//! panels and `SelectedBg` shared that fate through the scheme base.
//!
//! The fix derives each panel from the probed background — the family's hue, the
//! background's lightness plus one perceptual step (lighter on dark, darker on
//! light), and a floor of the background's own chroma so a tinted terminal keeps
//! its character. Same step for every family, so text contrast is equal across
//! panels and the state reads by hue alone.
//!
//! **One thing the issue text gets slightly wrong**, worth recording: it implies
//! the derivation should *mix* the family hue into the background at 10–18%. I
//! measured that and it does not work — 12% of green over Solarized's teal gives
//! `#184749`, which still reads teal, because a low-chroma background's hue
//! dominates. The family has to supply the hue outright, with the background
//! contributing chroma as a floor. The ratio is still pi's range, just applied to
//! the right axis.
//!
//! # Gap 2 — `COLORFGBG` 7/8 inverted
//!
//! The rule was `background >= 8 -> Light`, which classified ANSI 7
//! (white/light-gray) as Dark and ANSI 8 (bright black — Solarized Dark's own
//! background, `COLORFGBG=12;8`) as Light: backwards for both indices.
//!
//! pi's rule (`theme.ts`, `detectColorFgBgTheme`): `index <= 6 || index === 8`
//! is dark; `7` and `9..=15` are light; anything else is `undefined`.
//!
//! The behavioural rows live in `crates/lca-ui/src/theme/panels_tests.rs` and
//! `theme/mod.rs`'s `scheme_detection_tests`. This is the NFR-24 named guard: it
//! fails if either mechanism is quietly reverted.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

// Verifies: gh #387 gap 1 - **`system` derives panels instead of copying the
// background.** The old code is the defect, so its exact shape is what this row
// refuses to see again.
#[test]
fn the_system_palette_no_longer_copies_the_background() {
    let palette = read("crates/lca-ui/src/theme/palette.rs");
    let start = palette
        .find("pub fn system(")
        .expect("`Palette::system` exists");
    let rest = &palette[start..];
    let end = rest.find("\n    }\n").expect("`system` has a body");
    let body = &rest[..end];

    assert!(
        !body.contains("palette.set(Role::UserMessageBg, background)"),
        "setting a panel equal to the probed background is the invisible-card \
         defect: a band at the page's colour is not a band"
    );
    assert!(
        body.contains("panels::families()"),
        "every panel family is derived, not just the two message bands"
    );
    assert!(
        body.contains("panels::panel("),
        "the derivation is what paints them"
    );
    // `None` keeps transparency: no background, no invented colour.
    assert!(
        body.contains("unwrap_or(Color::Default)"),
        "a terminal that reported no background keeps every panel at `Default`, \
         so a translucent terminal is not painted over"
    );
}

// Verifies: gh #387 gap 1 - the derivation is the documented rule: a perceptual
// step whose sign follows the scheme, and the family's hue rather than a fixed
// hex. A constant step would make every theme look like one theme.
#[test]
fn the_derivation_is_perceptual_and_background_derived() {
    let panels = read("crates/lca-ui/src/theme/panels.rs");
    for needle in [
        "PANEL_STEP",
        "oklab_lightness",
        "rgb_to_oklab",
        "oklab_to_rgb",
        "ColorScheme::Dark => (bl + PANEL_STEP)",
        "ColorScheme::Light => (bl - PANEL_STEP)",
    ] {
        assert!(
            panels.contains(needle),
            "the derivation uses `{needle}`: a fixed hex step makes Solarized \
             read like Tokyo Night"
        );
    }
    // The neutral family carries no hue at all - giving it the blue hue made a
    // pending card identical to the user's band (found by its own test).
    assert!(
        panels.contains("chroma: 0.0"),
        "the neutral family is genuinely neutral, or it collides with another \
         family's colour"
    );
}

// Verifies: gh #387 gap 2 - **pi's classification, exactly.** Index 8 is bright
// black (dark); 7 is white (light). The old `>= 8` inverted both.
#[test]
fn colorfgbg_classifies_like_pi() {
    // The detector moved to its own module when `theme/mod.rs` crossed the
    // 1,200-line ceiling (gh #387 grew it); the guard follows the code.
    let module = read("crates/lca-ui/src/theme/scheme.rs");
    assert!(
        module.contains("index <= 6 || index == 8"),
        "pi's rule is `index <= 6 || index === 8` -> dark; anything else \
         (7, 9..=15) is light"
    );
    assert!(
        !module.contains("if background >= 8 {"),
        "the inverted rule must not come back"
    );
    // Both real-world reports the bug broke.
    let tests = read("crates/lca-ui/src/theme/scheme.rs");
    assert!(
        tests.contains(r#""12;8""#) && tests.contains(r#""0;7""#),
        "the Solarized Dark and xterm-light reports are pinned"
    );
    // Garbage is `None`, never a guess.
    assert!(
        module.contains("index > 15"),
        "an out-of-range index is rejected, not classified: a wrong scheme \
         silently repaints every panel"
    );
}

// Verifies: gh #387 gap 2 - both `COLORFGBG` shapes and the `default` literal:
// the background is the **last** field in `fg;bg` and `fg;xpm;bg`, and a
// non-numeric field is `None` (pi's `^\\d{1,2}$`).
#[test]
fn both_reports_shapes_are_handled() {
    let module = read("crates/lca-ui/src/theme/scheme.rs");
    assert!(
        module.contains("value.rsplit(';').next()"),
        "the background is the last field, which covers both shapes"
    );
    assert!(
        module.contains("is_ascii_digit"),
        "a literal `default` is rejected rather than read as 0"
    );
}

// Verifies: gh #387 gap 4 - the thinking border is a *ramp*, and its ordering
// is a property of the code rather than an accident of seven literals.
//
// A row that pinned the seven hex values would pass while the ramp is
// perceptually wrong, which is the trap this gap is about; so the guard checks
// the mechanism that makes the ordering hold:
//
//  - the seven levels are authored in OKLCH (`RAMPS` of hue+chroma plus a
//    per-scheme lightness ladder), not as seven hex strings;
//  - both `dark()` and `light()` fill their thinking roles *from* the ladder
//    (`apply_thinking`), so neither palette can quietly keep a stale table;
//  - the `system` palette re-derives from the probed background;
//  - `max` is not pi's error hue, which is the one place the ramp departs from
//    pi's table on purpose.
//
// The ordering maths itself lives in `theme::thinking`'s own rows; this is the
// NFR-24 guard that the wiring which produces it stays in place.
#[test]
fn the_thinking_border_is_a_ramp() {
    let module = read("crates/lca-ui/src/theme/thinking.rs");
    // Authored in OKLCH: seven `(hue, chroma)` pairs, not seven `#rrggbb`.
    assert!(
        module.contains("const RAMPS: [(f64, f64); 7]"),
        "the ramp is seven (hue, chroma) pairs in OKLCH - a literal hex table \
         is the defect this gap filed"
    );
    assert!(
        module.contains("const DARK_LIGHTNESS: [f64; 7]")
            && module.contains("const LIGHT_LIGHTNESS: [f64; 7]"),
        "each scheme owns a lightness ladder, so the light ramp steps down"
    );
    // The light scheme's chroma is a separate ladder: light surfaces sit at the
    // top of the gamut where less chroma is available, and reusing the dark
    // ladder clips every bright level.
    assert!(
        module.contains("const LIGHT_CHROMA: [f64; 7]"),
        "the light scheme's chroma is fitted to the gamut, not copied"
    );
    // The far end is flame, not the error hue.
    assert!(
        module.contains("(45.00, 0.185)"),
        "`max` ends at flame 45 degrees: pi's 20-degree `thinkingRed` is the \
         error red's own hue, so a border and an error would look identical"
    );
    // No leftover hex table for the thinking roles.
    assert!(
        !module.contains("\"#b294bb\"") && !module.contains("\"#ff5fff\""),
        "the hand-picked thinking hex table is gone from the ramp"
    );

    // Both named palettes fill from the ladder.
    let palette = read("crates/lca-ui/src/theme/palette.rs");
    let dark_calls = palette.matches("apply_thinking(None,").count();
    assert_eq!(
        dark_calls, 2,
        "`dark()` and `light()` each fill their thinking roles from the ramp \
         (found {dark_calls} calls)"
    );
    assert!(
        palette.contains("apply_thinking(background, scheme)"),
        "the `system` palette re-derives the ramp from the probed background"
    );
    // No `(Role::Thinking*, "#...")` pair survives. Matched as a pair rather
    // than as a bare hex: `#81a2be` is *also* the dark markdown-link colour, and
    // a substring scan for it would fail on a file that is entirely correct.
    for role in [
        "ThinkingOff",
        "ThinkingMinimal",
        "ThinkingLow",
        "ThinkingMedium",
        "ThinkingHigh",
        "ThinkingXhigh",
        "ThinkingMax",
    ] {
        assert!(
            !palette.contains(&format!("(Role::{role}, \"#")),
            "{role} still has a hand-picked hex value in `palette.rs`"
        );
    }

    // The border resolves the level through its role, so the ramp is painted.
    let separator = read("crates/lca-ui/src/separator.rs");
    assert!(
        separator.contains("Some(\"max\") => Role::ThinkingMax"),
        "the separator maps every level to its own role"
    );
}

// Verifies: gh #387 gap 5 - a theme's missing role resolves through a declared,
// same-slot parent rather than the ad-hoc `Text` guess, and the table that
// declares it is sound.
//
// The guard inspects the *mechanism*, because the property it protects is
// structural: a fallback must be semantically related and stay in its colour
// slot. `theme/tokens_tests.rs` owns the resolution maths (19 rows, five of
// which fail against the old `or_else(Text)`); this is the NFR-24 pin that the
// mechanism stays where it is.
#[test]
fn theme_roles_fall_back_through_declared_chains() {
    use lca_ui::theme::Role;
    use lca_ui::theme::tokens::{self, Slot};

    // One table, not a match arm per role scattered through the renderers - and
    // asked of the code, so `cargo fmt` reflowing a declaration cannot turn this
    // red for nothing.
    assert_eq!(
        tokens::TOKENS.len(),
        Role::ALL.len(),
        "the role->parent table is one const covering the whole vocabulary"
    );
    // Both halves of a declaration are present, so a role cannot be added with a
    // colour and no place in the chain: every role has a slot, and every role
    // that is not a slot root has a parent.
    for entry in tokens::TOKENS {
        for slot in [Slot::Foreground, Slot::Background] {
            if entry.slot == slot && entry.fallback.is_none() {
                assert_eq!(
                    tokens::root_of(slot),
                    Some(entry.role),
                    "the only parentless entry in {slot:?} is its root"
                );
            }
        }
    }
    let tokens_src = read("crates/lca-ui/src/theme/tokens.rs");
    // The same-slot invariant is enforced, not merely intended.
    assert!(
        tokens_src.contains("in a different slot"),
        "a fallback that crosses foreground/background is refused, the way pi \
         refuses it (`has fallback X in a different color slot`)"
    );
    // Cycles are refused with the loop named.
    assert!(
        tokens_src.contains("fallback cycle:"),
        "a declared cycle fails load naming the cycle"
    );
    // The slot roots are the two a theme is built up from. Asked of the code,
    // not of a line: `cargo fmt` reflows a struct literal across lines, and a
    // substring match on the unformatted form goes red for no reason (the same
    // trap that bit the gh #389 and gh #400 guards).
    assert_eq!(
        tokens::root_of(Slot::Foreground),
        Some(lca_ui::theme::Role::Text),
        "`text` is the foreground root"
    );
    assert_eq!(
        tokens::root_of(Slot::Background),
        Some(lca_ui::theme::Role::SelectedBg),
        "`selectedBg` is the background root"
    );

    // The palette resolves through the table instead of guessing `Text`, and
    // does so at load so the render path reads one value.
    let palette = read("crates/lca-ui/src/theme/palette.rs");
    assert!(
        palette.contains("tokens::resolve_chain(role,"),
        "`Palette::get` walks the declared chain, not `or_else(Text)`"
    );
    assert!(
        !palette.contains("or_else(|| self.colors.get(&Role::Text).copied())"),
        "the ad-hoc `Text` fallback is gone"
    );
    assert!(
        palette.contains("tokens::nearest_named("),
        "`overlay` resolves each role through the file's own declarations, which \
         is what makes a parent override carry its children"
    );
    // And the table is validated before it is trusted.
    assert!(
        palette.contains("tokens::validate()?"),
        "a malformed table fails load instead of painting a wrong colour"
    );
}
