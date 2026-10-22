//! Panel visibility for the `system` theme (gh #387 gap 1).
//!
//! The defect was that `UserMessageBg` and `CustomMessageBg` were set **equal
//! to** the probed terminal background — a band the same colour as the page it
//! sits on, which is no band at all. The tool panels and `SelectedBg` shared
//! that fate through the scheme base, so a user message and a tool card were
//! indistinguishable from the transcript.
//!
//! These rows measure rather than eyeball: a panel must differ from the
//! background by a *stated* perceptual amount, in the direction the scheme
//! needs, at **equal** text contrast to its siblings, carrying its family's
//! hue, and derived from the probed background rather than a constant.

use super::*;
use lca_tui::engine::colors::RgbColor;

/// Solarized Dark's background.
const SOLARIZED_DARK: RgbColor = RgbColor {
    r: 0x00,
    g: 0x2b,
    b: 0x36,
};
/// Tokyo Night's background.
const TOKYO_NIGHT: RgbColor = RgbColor {
    r: 0x1a,
    g: 0x1b,
    b: 0x26,
};
/// A light terminal background (xterm's white-ish default).
const LIGHT_BG: RgbColor = RgbColor {
    r: 0xf5,
    g: 0xf5,
    b: 0xf5,
};

/// The derived colour for one role, as `RgbColor`, or `None` for `Default`.
fn panel_of(
    background: RgbColor,
    scheme: ColorScheme,
    role: crate::theme::Role,
) -> Option<RgbColor> {
    let family = families().into_iter().find(|f| f.role == role)?;
    match panel(background, scheme, &family)? {
        Color::Rgb(r, g, b) => Some(RgbColor { r, g, b }),
        _ => None,
    }
}

/// WCAG relative luminance, for the contrast ratio.
fn luminance(color: RgbColor) -> f64 {
    let channel = |value: u8| {
        let v = f64::from(value) / 255.0;
        if v <= 0.039_28 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

/// WCAG contrast between two colours (1.0 = identical, 21.0 = black/white).
fn contrast(a: RgbColor, b: RgbColor) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

// Verifies: gh #387 gap 1 - **every panel is visibly different from the
// background.** The exact defect: equality. A band at the page's colour is not
// a band, and the old code set two of them to exactly that.
#[test]
fn every_panel_differs_from_the_background() {
    for (name, background, scheme) in [
        ("Solarized Dark", SOLARIZED_DARK, ColorScheme::Dark),
        ("Tokyo Night", TOKYO_NIGHT, ColorScheme::Dark),
        ("light", LIGHT_BG, ColorScheme::Light),
    ] {
        // Through `Palette::system`, not the derivation alone: the defect was
        // the *wiring* (panels set equal to the background), so a row that only
        // exercises `panel()` would keep passing with the old behaviour.
        let palette = crate::theme::Palette::system(Some(background), scheme);
        for family in families() {
            let derived = palette.get(family.role);
            let Color::Rgb(r, g, b) = derived else {
                panic!("{} on {name} is a concrete colour", family.role.key());
            };
            assert_ne!(
                (r, g, b),
                (background.r, background.g, background.b),
                "{} on {name} must not be the background itself",
                family.role.key()
            );
            // And not merely different by a rounding step: a measurable one.
            let l_bg = oklab_lightness(background);
            let l_panel = oklab_lightness(RgbColor { r, g, b });
            assert!(
                (l_panel - l_bg).abs() >= PANEL_STEP * 0.5,
                "{} on {name} moved {:.4} in OKLab L, which is not a visible step",
                family.role.key(),
                (l_panel - l_bg).abs()
            );
        }
    }
}

// Verifies: gh #387 gap 1 - **the step's direction follows the scheme.** Lighter
// on a dark background, darker on a light one. The wrong sign makes a panel
// recede into a background it was supposed to part from.
#[test]
fn the_step_direction_follows_the_scheme() {
    let l_bg = oklab_lightness(SOLARIZED_DARK);
    for family in families() {
        let derived = panel(SOLARIZED_DARK, ColorScheme::Dark, &family).expect("a panel");
        let Color::Rgb(r, g, b) = derived else {
            panic!("concrete")
        };
        assert!(
            oklab_lightness(RgbColor { r, g, b }) > l_bg,
            "{} is lighter than a dark background",
            family.role.key()
        );
    }
    let l_light = oklab_lightness(LIGHT_BG);
    for family in families() {
        let derived = panel(LIGHT_BG, ColorScheme::Light, &family).expect("a panel");
        let Color::Rgb(r, g, b) = derived else {
            panic!("concrete")
        };
        assert!(
            oklab_lightness(RgbColor { r, g, b }) < l_light,
            "{} is darker than a light background",
            family.role.key()
        );
    }
}

// Verifies: gh #387 gap 1 - **the user band and the tool cards are not the same
// shade.** They were, which is why a message and a card looked alike. Distinct
// *families* mean distinct colours on the same background.
#[test]
fn the_user_band_and_the_tool_cards_differ_from_each_other() {
    use crate::theme::Role;
    let user = panel_of(SOLARIZED_DARK, ColorScheme::Dark, Role::UserMessageBg).expect("user band");
    let pending =
        panel_of(SOLARIZED_DARK, ColorScheme::Dark, Role::ToolPendingBg).expect("pending card");
    let success =
        panel_of(SOLARIZED_DARK, ColorScheme::Dark, Role::ToolSuccessBg).expect("success card");
    let error = panel_of(SOLARIZED_DARK, ColorScheme::Dark, Role::ToolErrorBg).expect("error card");
    let custom =
        panel_of(SOLARIZED_DARK, ColorScheme::Dark, Role::CustomMessageBg).expect("custom band");
    for (a, b, what) in [
        (user, pending, "user vs pending"),
        (user, success, "user vs success"),
        (user, error, "user vs error"),
        (user, custom, "user vs custom"),
        (success, error, "success vs error"),
    ] {
        assert_ne!(a, b, "{what} must be different surfaces");
    }
}

// Verifies: gh #387 gap 1 - **contrast parity: no card is harder to read than
// another.** All panels move the same perceptual distance, so white (dark
// theme) or black (light theme) text has the same contrast on each. If this
// drifts, one state's message reads worse than another's for no reason.
#[test]
fn every_panel_has_the_same_text_contrast() {
    for (name, background, scheme, text) in [
        (
            "Solarized Dark",
            SOLARIZED_DARK,
            ColorScheme::Dark,
            RgbColor {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            },
        ),
        (
            "light",
            LIGHT_BG,
            ColorScheme::Light,
            RgbColor { r: 0, g: 0, b: 0 },
        ),
    ] {
        let ratios: Vec<(crate::theme::Role, f64)> = families()
            .into_iter()
            .map(|family| {
                let derived = panel_of(background, scheme, family.role).expect("a panel");
                (family.role, contrast(text, derived))
            })
            .collect();
        let first = ratios[0].1;
        for (role, ratio) in &ratios {
            assert!(
                (ratio - first).abs() < 0.35,
                "{} on {name} has {ratio:.2}:1 but {} has {first:.2}:1 - panels \
                 must be equally readable",
                role.key(),
                ratios[0].0.key()
            );
        }
        // …and every panel clears the body-text floor pi enforces.
        for (role, ratio) in &ratios {
            assert!(
                *ratio >= 4.5,
                "{} on {name} is {ratio:.2}:1, below the 4.5:1 body-text floor",
                role.key()
            );
        }
    }
}

// Verifies: gh #387 gap 1 - **the tint comes from the background, not a
// constant.** Two themes with very different backgrounds must derive different
// panels; a fixed hex step would make every theme look like one theme.
#[test]
fn the_tint_is_derived_from_the_background() {
    use crate::theme::Role;
    let solarized =
        panel_of(SOLARIZED_DARK, ColorScheme::Dark, Role::UserMessageBg).expect("solarized");
    let tokyo = panel_of(TOKYO_NIGHT, ColorScheme::Dark, Role::UserMessageBg).expect("tokyo");
    assert_ne!(
        solarized, tokyo,
        "the same family on two backgrounds is two colours"
    );
    // Solarized's background is blue-green; its blue band must keep that cast
    // rather than snapping to a canonical blue.
    assert!(
        f64::from(solarized.b) > f64::from(solarized.r),
        "Solarized's user band keeps the background's blue bias: {solarized:?}"
    );
}

// Verifies: gh #387 gap 1 - **the family hue survives the mix.** A green
// success band must still read green and a red error band red, or the state
// stops reading by colour alone.
#[test]
fn each_family_keeps_its_hue() {
    use crate::theme::Role;
    let success = panel_of(SOLARIZED_DARK, ColorScheme::Dark, Role::ToolSuccessBg).expect("green");
    let error = panel_of(SOLARIZED_DARK, ColorScheme::Dark, Role::ToolErrorBg).expect("red");
    let user = panel_of(SOLARIZED_DARK, ColorScheme::Dark, Role::UserMessageBg).expect("blue");
    assert!(
        success.g > success.r && success.g > success.b,
        "success reads green: {success:?}"
    );
    assert!(
        error.r > error.g && error.r > error.b,
        "error reads red: {error:?}"
    );
    assert!(user.b > user.r, "the user band reads blue: {user:?}");
}

// Verifies: gh #387 gap 1 - **`background: None` keeps transparency.** With
// nothing reported there is nothing to offset from; inventing a colour would
// paint a band the user did not ask for and break a translucent terminal.
#[test]
fn no_reported_background_keeps_every_panel_transparent() {
    // `Palette::system` is where this rule lives; the derivation itself is
    // never called without a background.
    let palette = crate::theme::Palette::system(None, ColorScheme::Dark);
    for family in families() {
        assert_eq!(
            palette.get(family.role),
            Color::Default,
            "{} stays Default when the terminal reports no background",
            family.role.key()
        );
    }
}

// Verifies: gh #387 gap 1 - the derivation is applied by `Palette::system`, not
// merely available: a probed background really produces shaded panels there.
#[test]
fn the_system_palette_uses_the_derivation() {
    use crate::theme::Role;
    let palette = crate::theme::Palette::system(Some(SOLARIZED_DARK), ColorScheme::Dark);
    let background = Color::Rgb(SOLARIZED_DARK.r, SOLARIZED_DARK.g, SOLARIZED_DARK.b);
    for family in families() {
        let color = palette.get(family.role);
        assert_ne!(
            color,
            background,
            "{} must not be the probed background itself",
            family.role.key()
        );
        assert_ne!(
            color,
            Color::Default,
            "{} is derived, not left transparent",
            family.role.key()
        );
    }
    // The foreground is still the terminal's own (gh #10's contract).
    assert_eq!(palette.get(Role::Text), Color::Default);
}
