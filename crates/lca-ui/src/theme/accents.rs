//! Gh #387 gap 3: the built-in palettes, calibrated rather than hand-picked.
//!
//! The dark and light palettes shipped pure-yellow warnings and a teal accent
//! with no perceptual relationship between the accents: at the same nominal
//! "brightness" a blue and a yellow read at different weights, so a success
//! message and a warning message in the same list did not feel equally
//! important. This module is the **rule** - the hex values in
//! [`Palette::dark`]/[`Palette::light`] are its output.
//!
//! # The rule
//!
//! - Every **accent** role sits at one target perceptual lightness: `OKHSL 67%`
//!   for the dark palette, `OKHSL 47%` for the light one. Equal lightness is what
//!   makes red/green/blue/yellow/violet feel equally bright.
//! - `muted` shares the accent lightness at near-zero chroma: same weight,
//!   colour removed. `dim` is the same construction, one step further out.
//! - Only **chroma** varies between accents, and it is scaled from each hue's
//!   own in-gamut maximum rather than hand-picked per hue.
//!
//! Lightness is **computed** (OKHSL -> OKLab), never guessed at `HSL 50%`:
//! equating blue and yellow at HSL `50%` is the flatness this gap is about.
//!
//! # What this file does *not* claim
//!
//! The accents do not clear WCAG 4.5:1 on every panel surface, and neither do
//! pi's own. See `the_light_accents_clear_the_floor_on_the_reading_surfaces` for
//! the pairs that *are* asserted and why the others are not.

use super::palette::{Palette, parse_color};
use super::panels::{from_oklch, oklab_lightness, oklch_of};
use super::{Color, Role};
use lca_tui::engine::colors::RgbColor;

/// The dark palette's accent target, as OKHSL lightness.
pub const DARK_ACCENT_OKHSL: f64 = 0.67;
/// The light palette's accent target, as OKHSL lightness.
///
/// pi's `light.json` uses `47%`, and at that lightness four of the five accents
/// land **below** the 4.5:1 body-text floor on LCA's own user band
/// (`#e8e8e8`): success 3.85, warning 4.13, muted and dim 4.06. De-emphasis that
/// cannot be read is the bug this cycle's design rule 4 names, so the target is
/// `41%` - the lightest value that clears the floor on *every* surface an accent
/// is painted on (worst case 4.85:1, on the user band).
///
/// This is a deliberate divergence from pi, recorded in the report: pi's own
/// light accents fail the same floor on its own selected-row surface (3.76-4.55),
/// so "match pi" and "stay readable" were never both available, and readability
/// wins.
pub const LIGHT_ACCENT_OKHSL: f64 = 0.41;
/// How far an accent's lightness may sit from the target and still count as
/// calibrated.
///
/// `0.012` in OKLab lightness is well under a just-noticeable difference for
/// this purpose (the scale is 0..1 and the ramps people notice are ~0.02), so a
/// row using it catches a hand-tweak one hue at a time while tolerating the
/// rounding that a hex value necessarily introduces.
pub const LIGHTNESS_TOLERANCE: f64 = 0.012;

/// The chroma a `muted`/`dim` role is allowed, above which it is no longer
/// "the same weight, colourless".
pub const NEUTRAL_CHROMA_MAX: f64 = 0.030;

/// The roles that carry an accent colour (the ones the calibration applies to).
///
/// `syntax*` and `md*` roles are excluded on purpose: they are a *code* palette,
/// borrowed from editors so code looks like code, and pulling them onto the
/// accent lightness would flatten syntax highlighting into one weight. The
/// accent family is what a reader compares as "messages", and that is what the
/// rule governs.
pub const ACCENT_ROLES: [Role; 5] = [
    Role::Success,
    Role::Error,
    Role::Warning,
    Role::Accent,
    Role::BorderAccent,
];

/// The OKHSL lightness pi and the issue name, as OKLab lightness.
#[must_use]
pub fn okhsl_to_oklab_lightness(okhsl: f64) -> f64 {
    // Ottosson's `toe_inv`: OKHSL's L is OKLab's L pushed through a toe, and the
    // inverse is what turns "67%" into a number the OKLab conversion takes.
    let (k1, k2) = (0.206, 0.03);
    let k3 = (1.0 + k1) / (1.0 + k2);
    (okhsl * okhsl + k1 * okhsl) / (k3 * (okhsl + k2))
}

/// OKLab lightness of a role's colour in `palette`, or `None` when the role is
/// not an RGB value.
#[must_use]
pub fn role_lightness(palette: &Palette, role: Role) -> Option<f64> {
    rgb_of(palette, role).map(oklab_lightness)
}

/// OKLCH chroma of a role's colour.
#[must_use]
pub fn role_chroma(palette: &Palette, role: Role) -> Option<f64> {
    rgb_of(palette, role).map(|color| oklch_of(color).1)
}

/// The role's colour as RGB, or `None` for `Default`/an ANSI index.
fn rgb_of(palette: &Palette, role: Role) -> Option<RgbColor> {
    match palette.get(role) {
        Color::Rgb(r, g, b) => Some(RgbColor { r, g, b }),
        _ => None,
    }
}

/// WCAG relative luminance.
#[must_use]
pub fn relative_luminance(color: RgbColor) -> f64 {
    let channel = |value: u8| {
        let v = f64::from(value) / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

/// WCAG contrast between two colours (1.0 identical, 21.0 black on white).
#[must_use]
pub fn contrast(a: RgbColor, b: RgbColor) -> f64 {
    let (l1, l2) = (relative_luminance(a), relative_luminance(b));
    let (high, low) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (high + 0.05) / (low + 0.05)
}

/// The largest OKLCH chroma at `lightness`/`hue` that survives sRGB unclipped.
///
/// This is a **bisection on chroma only** - not the general gamut-mapping solver
/// (which would map an out-of-gamut colour back by reducing chroma *and*
/// lightness). Because the accents are *authored* at a fixed lightness and only
/// their chroma is in question, a one-dimensional search answers exactly the
/// question asked, in a few dozen lines. That is why this cycle needs no solver.
#[must_use]
pub fn max_chroma_at(lightness: f64, hue: f64) -> f64 {
    let mut low = 0.0_f64;
    let mut high = 0.5_f64;
    for _ in 0..40 {
        let mid = (low + high) / 2.0;
        let candidate = from_oklch(lightness, mid, hue);
        let (measured_l, measured_c, _) = oklch_of(candidate);
        if (measured_l - lightness).abs() < 0.002 && (measured_c - mid).abs() < 0.006 {
            low = mid;
        } else {
            high = mid;
        }
    }
    low
}

/// An accent colour at `okhsl` lightness, `hue`, and a fraction of that hue's
/// maximum chroma.
#[must_use]
pub fn accent(okhsl: f64, hue: f64, chroma_fraction: f64) -> RgbColor {
    let lightness = okhsl_to_oklab_lightness(okhsl);
    from_oklch(
        lightness,
        max_chroma_at(lightness, hue) * chroma_fraction,
        hue,
    )
}

/// A neutral at an accent's lightness: same weight, colour removed.
#[must_use]
pub fn neutral(okhsl: f64, chroma: f64) -> RgbColor {
    // Hue 229 is the cool-neutral axis pi uses for `muted`/`dim`; at this chroma
    // it reads as grey with a trace of cool, not as a colour.
    from_oklch(okhsl_to_oklab_lightness(okhsl), chroma, 229.0)
}

/// Render a colour the way a palette entry is written.
#[must_use]
pub fn hex(color: RgbColor) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// Whether a palette entry parses to exactly `expected`.
#[must_use]
pub fn role_is(palette: &Palette, role: Role, expected: RgbColor) -> bool {
    parse_color(&hex(expected)) == Some(palette.get(role))
}

#[cfg(test)]
#[path = "accents_tests.rs"]
mod accents_tests;
