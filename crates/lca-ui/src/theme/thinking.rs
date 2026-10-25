//! The thinking-level heat ramp (gh #387 gap 4).
//!
//! Seven levels, one ambient signal: how much reasoning budget a turn is
//! spending, painted on the editor border
//! (`separator::separator_border` → `chat_render.rs:170` and the footer). The
//! values used to be hand-picked hex with no ordering relationship at all —
//! measured in OKLCH, the dark palette's last four steps moved only 0.01–0.02 in
//! lightness and two of them had *identical* hue (318.33° → 318.24°), so
//! "xhigh" and "high" were the same colour to the eye. The light palette was
//! worse: its lightness ran 0.757 → 0.566 → 0.577 → 0.572 → 0.542 → 0.447 →
//! 0.490, i.e. up and down, and its hue *reversed* at two levels.
//!
//! # What this module promises
//!
//! Ordered on three axes, in OKLCH, so the guard can check the *ordering*
//! rather than seven literals:
//!
//! 1. **Hue rotates cool → hot, monotonically.** Slate (231°) → periwinkle
//!    (263°) → violet (295°) → magenta (337°) → flame (45°). The first three
//!    levels share the slate hue, which is not a bug: pi's `thinkingSlate`,
//!    `thinkingBlue` and `thinkingPeriwinkle` families are 231.49°, 231.49°,
//!    263.25°, so "off/minimal/low" differ by saturation, and the rotation
//!    starts at `medium`. Rotating *within* a family would make `low` a
//!    different colour from `minimal` while saying the same thing.
//! 2. **Chroma strictly increases** across all seven, so the hottest level is
//!    always the most saturated.
//! 3. **Lightness moves monotonically away from the background** — up on dark,
//!    down on light. "Increasing" would be wrong: on a light terminal a ramp
//!    that got lighter would fade *into* the background, which is the defect
//!    this issue filed, not the fix.
//!
//! # Why the literals, and not pi's polynomial
//!
//! pi solves each level's lightness from a per-level curve fitted to the
//! background's OKLab `L` (`system-theme.ts:150-195`), then paints a family at
//! that lightness. Evaluating those curves here gives a fixed step of ~0.021 in
//! `L` per level on either scheme — the same ladder, from six coefficients. This
//! module ships the ladder its authored endpoints imply (0.049 → 0.040 on dark,
//! −0.061 → −0.052 on light) with the *shape* of pi's ramp, measured in the same
//! space.
//!
//! The one place I departed from pi's table on purpose: pi's `thinkingRed` is
//! hue 20°, which is the **error red's own hue** (`Role::Error` is `#cc6666` =
//! 21.4°). At `max` thinking, a border and an error would be the same colour —
//! and the border would stop meaning one thing. The ramp ends at flame 45°
//! instead, 24° clear of the error hue, which is still "red-orange" and is what
//! `max` reads as.
//!
//! # The system palette
//!
//! A ramp that only works on a dark terminal is half a fix, so the `system`
//! palette derives its own ladder from the *probed background*: same hues, same
//! chroma, and lightness stepping away from that background by the same
//! proportions. On a Solarized Dark terminal that starts from 0.267 instead of
//! `dark()`'s nominal, so the ramp still ends where the eye expects. With no
//! reported background there is nothing to step away from and the levels keep
//! the scheme's values — which is why `system(None, …)` remains ordered.

use lca_tui::engine::colors::ColorScheme;

use super::Color;

/// The six OKLCH degrees between the seven levels' hues, as `(hue, chroma)`.
///
/// Chroma is the peak for its level, tuned per scheme by
/// [`scheme_ladders`]; the hue is shared, because hue is what "which step of the
/// heat scale is this" reads as (the report has the numbers).
const RAMPS: [(f64, f64); 7] = [
    (231.49, 0.012),
    (231.49, 0.045),
    (231.49, 0.090),
    (263.25, 0.130),
    (295.00, 0.155),
    (337.50, 0.172),
    (45.00, 0.185),
];

/// The light scheme's peak chroma per level.
///
/// Light surfaces sit at the top of the sRGB gamut, where less chroma is
/// available at usable lightness: at `L = 0.528`, hue 45° can hold at most
/// `C ≈ 0.15` before it clips. These are the dark ladder scaled to fit that
/// ceiling (`theme/thinking.rs`'s report entry has the solved limits).
const LIGHT_CHROMA: [f64; 7] = [0.012, 0.045, 0.080, 0.100, 0.118, 0.132, 0.145];

/// The dark ladder's OKLab lightness per level, authored from `dark()`'s base.
const DARK_LIGHTNESS: [f64; 7] = [0.440, 0.490, 0.540, 0.585, 0.630, 0.670, 0.710];

/// The light ladder's OKLab lightness per level, authored from `light()`'s base.
///
/// Gh #387 gap 3 moved the light error red onto the calibrated accent lightness
/// (`OKHSL 41%`, L 0.491) and gave it more chroma, which closed the gap the
/// `max` step had been relying on. The ladder is re-spaced to end at **0.545**
/// (an even 0.0525 step throughout), so the border's hottest level is again
/// distinguishable from an error by lightness alone - the gap is 0.054, over the
/// 0.05 the row asks for. Hue was never the problem: `max` sits 25 degrees from
/// the error red, which the other row pins.
const LIGHT_LIGHTNESS: [f64; 7] = [0.860, 0.807, 0.755, 0.702, 0.650, 0.597, 0.545];

/// How far the `system` ramp sits from the probed background at its outer end.
///
/// The same figure `panels::PANEL_STEP` uses for one panel, so the border's
/// `off` end reads as strongly as a card's edge rather than as a hairline that
/// happens to be off by a bit more.
const SYSTEM_REACH: f64 = 0.36;

/// One level's derived colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    /// The colour the border paints.
    pub color: Color,
    /// OKLab lightness, for the ordering rows.
    pub lightness: f64,
    /// OKLCH chroma, for the ordering rows.
    pub chroma: f64,
    /// OKLCH hue in degrees, for the ordering rows.
    pub hue: f64,
}

/// The seven levels in order, from `off` to `max`.
///
/// `background` is the terminal's reported background when there is one; the
/// ladder then steps away from *it* instead of from the scheme's nominal base.
#[must_use]
pub fn levels(
    background: Option<lca_tui::engine::colors::RgbColor>,
    scheme: ColorScheme,
) -> [Level; 7] {
    let (base_lightness, chromas, direction) = match scheme {
        ColorScheme::Dark => (&DARK_LIGHTNESS, &RAMPS.map(|(_, c)| c), 1.0),
        ColorScheme::Light => (&LIGHT_LIGHTNESS, &LIGHT_CHROMA, -1.0),
    };
    // Where the ramp starts: the probed background when the terminal answered,
    // otherwise the scheme's own outer level.
    let start = match background {
        Some(bg) => super::panels::oklab_lightness_of(bg),
        None => base_lightness[0],
    };
    // Step away from the start by the authored ladder's own offsets, scaled so
    // the far end reaches `SYSTEM_REACH` further out. With no background this
    // is the identity (`start` *is* `base_lightness[0]`).
    let outer = base_lightness[6];
    let authored_reach = (outer - base_lightness[0]).abs();
    let scale = if authored_reach > f64::EPSILON {
        SYSTEM_REACH / authored_reach
    } else {
        1.0
    };
    // A `system` session uses the probed ramp; a named theme uses its own.
    let use_probe = background.is_some();
    let mut out = [Level {
        color: Color::Default,
        lightness: 0.0,
        chroma: 0.0,
        hue: 0.0,
    }; 7];
    for (index, (hue, _)) in RAMPS.iter().enumerate() {
        let authored = base_lightness[index];
        let lightness = if use_probe {
            start + direction * (authored - base_lightness[0]).abs() * scale
        } else {
            authored
        };
        let chroma = chromas[index];
        let color = super::panels::from_oklch(lightness.clamp(0.0, 1.0), chroma, *hue);
        let (lightness, chroma, hue) = super::panels::oklch_of(color);
        out[index] = Level {
            color: Color::Rgb(color.r, color.g, color.b),
            lightness,
            chroma,
            hue,
        };
    }
    out
}

#[cfg(test)]
#[path = "thinking_tests.rs"]
mod tests;
