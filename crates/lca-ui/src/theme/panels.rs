//! Panel derivation for the `system` theme (gh #387 gap 1).
//!
//! # The defect
//!
//! `Palette::system` set `UserMessageBg` and `CustomMessageBg` **equal to** the
//! probed terminal background, and the tool panels and `SelectedBg` inherited
//! the same fate through the scheme base. A band whose background is the
//! transcript's background is not a band: a user message and a tool card were
//! indistinguishable from the text around them, which is the one thing a
//! message card exists to prevent.
//!
//! # The rule
//!
//! Take the probed background, mix the family's hue into it at a small ratio,
//! and move **one perceptual step** away from it — lighter on a dark
//! background, darker on a light one. Perceptual, not a fixed hex step: a fixed
//! step makes Solarized read like Tokyo Night, because the same `+0x10` on
//! `#002b36` and `#1a1b26` is a different amount of visible change and carries
//! no hue. Mixing at 12% keeps Solarized's blue-green surfaces blue-green.
//!
//! The sign is what makes the shade *visible*; the hue is what makes it *this
//! theme's*. Both come from the reported background, so nothing here is a
//! constant.
//!
//! # `background: None`
//!
//! Nothing to offset from, so every panel stays [`Color::Default`] and the
//! terminal's own background shows through. That is transparency the user chose
//! (a translucent terminal), and inventing a colour would paint a band they did
//! not ask for.
//!
//! # Contrast parity
//!
//! All panels move the **same** perceptual distance from the background, so
//! white-on-panel (or black-on-panel) contrast is equal across them: no card is
//! harder to read than its neighbour. Only the hue differs, which is what lets
//! the state read by colour alone.

use lca_tui::engine::colors::{ColorScheme, RgbColor};

use crate::theme::Color;

/// Panel chroma at rest, in OKLCH (0 = grey, ~0.15 is vivid).
///
/// This is a *band behind text*, not a highlight: enough chroma that the family
/// reads by hue (0.045 is clearly green/red/blue against a neutral), little
/// enough that a card never competes with the text on it.
pub(crate) const PANEL_CHROMA: f64 = 0.045;

/// How much of the background's own chroma is kept alongside the family hue.
///
/// pi mixes its surface tint at 10-18%. Averaging hues does not work here —
/// measured: 12% of green over Solarized's teal still renders `#184749`, which
/// reads teal, not green, because the background's hue dominates at low chroma.
/// So the family supplies the **hue** and the background supplies a floor of its
/// own chroma, which keeps Solarized surfaces looking like Solarized while the
/// state still reads by hue.
pub(crate) const BACKGROUND_CHROMA_KEEP: f64 = 0.35;

/// How far a panel moves from the background, in OKLab `L` (0..=1).
///
/// `0.06` in OKLab lightness is roughly one visible step on a dark background —
/// clearly a different surface, without becoming a coloured box. Small on
/// purpose: the panel is a band behind text, not a highlight.
pub(crate) const PANEL_STEP: f64 = 0.06;

/// One panel family: the role it paints and the hue it leans toward.
///
/// The hue is a reference point, not a fixed colour: it is mixed *into the
/// probed background*, so the result belongs to the terminal's palette. The
/// families are chosen so the state reads by hue alone at equal contrast —
/// blue for "you said this", green for success, red for failure, violet for
/// custom, neutral for "waiting".
pub(crate) struct PanelFamily {
    /// The role this family paints.
    pub role: crate::theme::Role,
    /// Reference hue in OKLCH degrees (0 = red, ~264 = blue).
    pub hue: f64,
    /// This family's resting chroma. The neutral family is **0**: "nothing has
    /// happened yet" is the absence of a state hue, and giving it the blue hue
    /// made a pending card the same colour as the user's own band.
    pub chroma: f64,
}

/// OKLab lightness of a colour (0 = black, 1 = white).
///
/// OKLab rather than HSL luminance: the step has to be *perceptually* even, or
/// a panel on a dark background would be a barely-visible nudge and the same
/// step on a light one a stark box.
// Reached only from tests (`panels_tests`, and `run_tests`'s OSC 11 row): the
// derivation itself uses the full OKLab triple, and a production caller would
// want the run to publish it rather than a private helper.
#[cfg(test)]
pub(crate) fn oklab_lightness(color: RgbColor) -> f64 {
    let (l, _, _) = rgb_to_oklab(color);
    l
}

/// sRGB (0..=1, gamma-encoded) → OKLab. The standard conversion: linearize,
/// LMS, cube roots, then the fixed matrix.
fn rgb_to_oklab(color: RgbColor) -> (f64, f64, f64) {
    let linear = |channel: u8| {
        let v = f64::from(channel) / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (linear(color.r), linear(color.g), linear(color.b));
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());
    (
        0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    )
}

/// OKLab → sRGB, clamped into gamut. The inverse of [`rgb_to_oklab`].
fn oklab_to_rgb(l: f64, a: f64, b: f64) -> RgbColor {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let b = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    let encode = |v: f64| {
        let v = v.clamp(0.0, 1.0);
        let out = if v <= 0.003_130_8 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (out * 255.0).round().clamp(0.0, 255.0) as u8
    };
    RgbColor {
        r: encode(r),
        g: encode(g),
        b: encode(b),
    }
}

/// Derive one panel colour: the probed background's lightness, stepped; the
/// family's hue; and enough of the background's own chroma that a tinted theme
/// keeps its cast.
///
/// `None` when the terminal reported no background — there is nothing to offset
/// from, and the caller keeps [`Color::Default`].
#[must_use]
pub(crate) fn panel(
    background: RgbColor,
    scheme: ColorScheme,
    family: &PanelFamily,
) -> Option<Color> {
    let (bl, ba, bb) = rgb_to_oklab(background);
    // The step: one perceptual notch away from the background, in the direction
    // that is *visible* — lighter on dark, darker on light. Clamped so a
    // pure-black or pure-white background cannot lose the panel entirely.
    let stepped = match scheme {
        ColorScheme::Dark => (bl + PANEL_STEP).min(1.0),
        ColorScheme::Light => (bl - PANEL_STEP).max(0.0),
    };
    // The background's chroma is the floor, not the mix: a tinted terminal
    // (Solarized's teal, Gruvbox's brown) keeps that character, and a neutral
    // one contributes nothing.
    let background_chroma = ba.hypot(bb) * BACKGROUND_CHROMA_KEEP;
    let chroma = family.chroma.max(background_chroma);
    let radians = family.hue.to_radians();
    let rgb = oklab_to_rgb(stepped, chroma * radians.cos(), chroma * radians.sin());
    if rgb == background {
        return None;
    }
    Some(Color::Rgb(rgb.r, rgb.g, rgb.b))
}

/// The families the `system` theme shades, in the order the issue lists them.
///
/// Every one gets the same treatment at the same step, which is what makes the
/// contrast parity hold: the state reads by hue, not by how hard it is to read.
#[must_use]
pub(crate) fn families() -> Vec<PanelFamily> {
    use crate::theme::Role;
    vec![
        // Blue: "this is what you said".
        PanelFamily {
            role: Role::UserMessageBg,
            hue: 264.0,
            chroma: PANEL_CHROMA,
        },
        // Blue, a shade cooler: the hover/selection wash means "pointing at
        // yours too", so it shares the family but not the exact value.
        PanelFamily {
            role: Role::SelectedBg,
            hue: 238.0,
            chroma: PANEL_CHROMA,
        },
        // **Neutral**: nothing has happened yet. Chroma 0 by design - a pending
        // card has no state to signal, and reusing the blue hue made it
        // indistinguishable from the user's own band.
        PanelFamily {
            role: Role::ToolPendingBg,
            hue: 264.0,
            chroma: 0.0,
        },
        // Green: it worked.
        PanelFamily {
            role: Role::ToolSuccessBg,
            hue: 145.0,
            chroma: PANEL_CHROMA,
        },
        // Red: it did not.
        PanelFamily {
            role: Role::ToolErrorBg,
            hue: 25.0,
            chroma: PANEL_CHROMA,
        },
        // Violet: something the harness composed - not you, not a tool.
        PanelFamily {
            role: Role::CustomMessageBg,
            hue: 310.0,
            chroma: PANEL_CHROMA,
        },
    ]
}

#[cfg(test)]
#[path = "panels_tests.rs"]
mod tests;
