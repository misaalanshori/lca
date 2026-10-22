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
