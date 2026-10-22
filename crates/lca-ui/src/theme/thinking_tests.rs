//! The thinking ramp's ordering rows (gh #387 gap 4).
//!
//! Every row here measures in OKLCH rather than pinning hex, because that is the
//! whole point of the gap: seven literals can be "a ramp" in a table and not be
//! one to the eye, and a row that asserts `#b294bb` passes either way. The old
//! values are the proof - see `the_old_hand_picked_values_were_not_a_ramp`.

use lca_tui::engine::colors::{ColorScheme, RgbColor};

use super::{Level, levels};
use crate::theme::panels::oklch_of;
use crate::theme::{Color, Palette, Role};

/// The seven roles, in the same order `levels` returns.
const ROLES: [Role; 7] = [
    Role::ThinkingOff,
    Role::ThinkingMinimal,
    Role::ThinkingLow,
    Role::ThinkingMedium,
    Role::ThinkingHigh,
    Role::ThinkingXhigh,
    Role::ThinkingMax,
];

/// An unwrapped hue sequence: each step's smallest forward turn from the last.
///
/// Raw hue is circular, so `350 -> 10` is a 20-degree step forward, not a
/// 340-degree step back. Unwrapping first is what lets "the hue rotates one
/// way" be a `windows(2).all(>)` rather than a sign-flip detector.
fn unwrapped_hues(levels: &[Level; 7]) -> Vec<f64> {
    let mut out = vec![levels[0].hue];
    for level in &levels[1..] {
        let previous = *out.last().expect("seeded above");
        out.push(previous + (level.hue - previous).rem_euclid(360.0));
    }
    out
}

/// Assert the three ordering properties for one ladder.
fn assert_ordered(label: &str, scheme: ColorScheme, ladder: &[Level; 7], away_from: f64) {
    let light = matches!(scheme, ColorScheme::Light);

    // 1. Lightness moves monotonically *away* from the background. Away, not
    //    up: on a light terminal a ramp that got lighter would fade into the
    //    page, which is the defect, not the fix.
    for pair in ladder.windows(2) {
        let step = pair[1].lightness - pair[0].lightness;
        if light {
            assert!(
                step < 0.0,
                "{label}: level lightness must fall on a light background, got {step:+.4} \
                 ({} -> {})",
                pair[0].lightness,
                pair[1].lightness
            );
        } else {
            assert!(
                step > 0.0,
                "{label}: level lightness must rise on a dark background, got {step:+.4} \
                 ({} -> {})",
                pair[0].lightness,
                pair[1].lightness
            );
        }
    }
    // The last level is the furthest from the page, on both schemes.
    let spread = (ladder[6].lightness - away_from).abs();
    let first = (ladder[0].lightness - away_from).abs();
    assert!(
        spread > first,
        "{label}: `max` ({spread:.3}) must sit further from the background than `off` \
         ({first:.3})"
    );

    // 2. Chroma strictly increases: the hottest level is the most saturated.
    for (index, pair) in ladder.windows(2).enumerate() {
        assert!(
            pair[1].chroma > pair[0].chroma,
            "{label}: chroma must rise at step {index} ({} -> {})",
            pair[0].chroma,
            pair[1].chroma
        );
    }

    // 3. Hue rotates one way through at least 120 degrees, so `off` and `max`
    //    read as different colours rather than two shades of one.
    let hues = unwrapped_hues(ladder);
    for pair in hues.windows(2) {
        assert!(
            pair[1] > pair[0],
            "{label}: hue must rotate forward, got {pair:?}"
        );
    }
    let rotation = hues[6] - hues[0];
    assert!(
        rotation >= 120.0,
        "{label}: the ramp must span at least 120 degrees of hue, got {rotation:.1}"
    );

    // The steps must be *even enough* to read as a ladder. The old dark palette
    // had a 11.7x ratio between its largest and smallest step; 2x is the most a
    // ramp can be uneven and still look like one.
    let steps: Vec<f64> = ladder
        .windows(2)
        .map(|pair| (pair[1].lightness - pair[0].lightness).abs())
        .collect();
    let smallest = steps.iter().copied().fold(f64::INFINITY, f64::min);
    let largest = steps.iter().copied().fold(0.0, f64::max);
    assert!(
        largest / smallest <= 2.0,
        "{label}: lightness steps must be even, got {smallest:.4}..{largest:.4} \
         ({}x)",
        largest / smallest
    );
}

#[test]
fn the_dark_palette_ramp_is_ordered() {
    let ladder = levels(None, ColorScheme::Dark);
    // `dark()`'s nominal page is not a role; the ramp's own `off` end is where
    // the palette starts, so "away from" is measured against the darkest text
    // the scheme paints with (`Role::Text`, #d4d4d4) inverted -- the page it is
    // drawn on. Simpler and honest: measure `max` against `off`.
    assert_ordered("dark", ColorScheme::Dark, &ladder, ladder[0].lightness);
}

#[test]
fn the_light_palette_ramp_is_ordered() {
    let ladder = levels(None, ColorScheme::Light);
    assert_ordered("light", ColorScheme::Light, &ladder, ladder[0].lightness);
}

#[test]
fn the_ramp_is_actually_painted_by_the_palettes() {
    for (label, palette, scheme) in [
        ("dark", Palette::dark(), ColorScheme::Dark),
        ("light", Palette::light(), ColorScheme::Light),
    ] {
        let expected = levels(None, scheme);
        for (role, level) in ROLES.iter().zip(expected) {
            assert_eq!(
                palette.get(*role),
                level.color,
                "{label}: {role:?} must hold the ramp's colour, not a leftover literal"
            );
        }
        // And the seven are distinct: a palette that set all seven from one
        // level would satisfy the per-role equality above only by accident.
        let mut seen: Vec<String> = ROLES
            .iter()
            .map(|role| format!("{:?}", palette.get(*role)))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 7, "{label}: all seven levels are distinct");
    }
}

#[test]
fn the_endpoints_are_far_apart_in_hue_and_saturation() {
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let ladder = levels(None, scheme);
        let hues = unwrapped_hues(&ladder);
        let rotation = hues[6] - hues[0];
        assert!(
            rotation >= 120.0,
            "{scheme:?}: `off` -> `max` must turn the hue, got {rotation:.1} degrees"
        );
        let saturation_gain = ladder[6].chroma - ladder[0].chroma;
        assert!(
            saturation_gain >= 0.10,
            "{scheme:?}: `max` must be clearly more saturated than `off`, got \
             {saturation_gain:.3}"
        );
    }
}

/// `max` thinking and an error state must not be the same colour, or the border
/// stops meaning one thing.
///
/// This is where the ramp deliberately departs from pi: pi's `thinkingRed` is
/// hue 20, and `Role::Error` is `#cc6666` = 21.4 degrees. A `max` at pi's hue
/// would be the error red on the border.
#[test]
fn max_thinking_is_not_the_error_colour() {
    for (label, palette, _scheme) in [
        ("dark", Palette::dark(), ColorScheme::Dark),
        ("light", Palette::light(), ColorScheme::Light),
    ] {
        let Color::Rgb(r, g, b) = palette.get(Role::ThinkingMax) else {
            panic!("{label}: `max` is a concrete colour");
        };
        let (max_l, max_c, max_h) = oklch_of(RgbColor { r, g, b });
        let Color::Rgb(er, eg, eb) = palette.get(Role::Error) else {
            panic!("{label}: error is a concrete colour");
        };
        let (err_l, err_c, err_h) = oklch_of(RgbColor {
            r: er,
            g: eg,
            b: eb,
        });
        let hue_gap = (max_h - err_h).rem_euclid(360.0);
        let hue_gap = hue_gap.min(360.0 - hue_gap);
        assert!(
            hue_gap >= 15.0,
            "{label}: `max` (hue {max_h:.1}) must clear the error hue \
             ({err_h:.1}) by 15 degrees, got {hue_gap:.1}"
        );
        assert!(
            max_c >= err_c || (max_l - err_l).abs() >= 0.05,
            "{label}: `max` must differ from error in chroma or lightness too, got \
             C {max_c:.3} vs {err_c:.3}, L {max_l:.3} vs {err_l:.3}"
        );
    }
}

/// The border must be *visible* on its page, both ends of the ramp.
///
/// A border is not body text, so this asserts the strict thing it needs: a
/// measurable lightness distance from the page it is drawn on.
#[test]
fn every_level_is_visible_against_its_background() {
    // The bar is "a hairline you can find", not "body text": a border glyph is
    // one pixel wide, so 0.08 of OKLab lightness is the point at which it stops
    // disappearing into the page. `off` is deliberately the quietest level, so
    // it sets the floor rather than some mid step.
    const VISIBLE: f64 = 0.08;
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let ladder = levels(None, scheme);
        let page = match scheme {
            ColorScheme::Dark => 0.10,  // a typical dark terminal page
            ColorScheme::Light => 0.97, // a typical light one
        };
        for (index, level) in ladder.iter().enumerate() {
            let distance = (level.lightness - page).abs();
            assert!(
                distance >= VISIBLE,
                "{scheme:?} level {index}: {distance:.3} from the page is too faint to see"
            );
        }
    }
    // And the old light palette failed this: its `max` was 0.490 against a page
    // of 0.97 -- 0.48 away, but the levels *between* dipped to 0.447, so the
    // ramp crossed the page's own darkness twice. Monotonicity is what fixes
    // that, and `assert_ordered` owns it.
}

/// The old values, measured - the receipt for "this was not a ramp".
///
/// Kept as a row rather than a sentence in the commit message so the claim can
/// be re-run. It fails if someone reintroduces a flat run of levels.
#[test]
fn the_old_hand_picked_values_were_not_a_ramp() {
    // The seven dark values this module replaced.
    let old = [
        ("#505050", 0.4313_f64),
        ("#6e6e6e", 0.5382),
        ("#5f87af", 0.6099),
        ("#81a2be", 0.6975),
        ("#b294bb", 0.7066),
        ("#d183e8", 0.7278),
        ("#ff5fff", 0.7495),
    ];
    // The tail is the defect: four levels inside 0.043 of lightness.
    let tail = old[6].1 - old[3].1;
    assert!(
        tail < 0.06,
        "the recorded old tail is {tail:.3}; if this changed, re-measure the claim \
         in the module docs"
    );
    // And two of them shared a hue outright.
    let high = oklch_of(RgbColor {
        r: 0xb2,
        g: 0x94,
        b: 0xbb,
    });
    let xhigh = oklch_of(RgbColor {
        r: 0xd1,
        g: 0x83,
        b: 0xe8,
    });
    assert!(
        (high.2 - xhigh.2).abs() < 0.5,
        "the old `high`/`xhigh` hues were {} and {}",
        high.2,
        xhigh.2
    );
    // The replacement does not have that tail.
    let now = levels(None, ColorScheme::Dark);
    let new_tail = now[6].lightness - now[3].lightness;
    assert!(
        new_tail > 0.10,
        "the new `medium` -> `max` span is {new_tail:.3}, which is not a ramp"
    );
}

/// With a probed background the ramp steps away from *that*, not from the
/// scheme's nominal page - otherwise a Solarized Dark terminal gets a ramp
/// anchored to a #343541 page it does not have.
#[test]
fn the_system_ramp_steps_away_from_the_probed_background() {
    let solarized = RgbColor {
        r: 0x00,
        g: 0x2b,
        b: 0x36,
    };
    let bg_l = crate::theme::panels::oklab_lightness_of(solarized);
    let system = levels(Some(solarized), ColorScheme::Dark);
    let plain = levels(None, ColorScheme::Dark);
    assert_ordered("system/solarized", ColorScheme::Dark, &system, bg_l);
    assert!(
        (system[0].lightness - bg_l).abs() < 0.08,
        "the ramp starts near the reported background ({} vs {bg_l:.3})",
        system[0].lightness
    );
    assert!(
        (system[6].lightness - plain[6].lightness).abs() > 0.02
            || (system[0].lightness - plain[0].lightness).abs() > 0.02,
        "the system ramp is anchored differently from the named one, otherwise \
         the probe does nothing"
    );
}

/// A light terminal with a probed background steps *down*, so the ramp never
/// fades into the page.
#[test]
fn the_system_ramp_falls_away_from_a_light_background() {
    let solarized_light = RgbColor {
        r: 0xfd,
        g: 0xf6,
        b: 0xe3,
    };
    let bg_l = crate::theme::panels::oklab_lightness_of(solarized_light);
    let system = levels(Some(solarized_light), ColorScheme::Light);
    assert_ordered("system/solarized-light", ColorScheme::Light, &system, bg_l);
    assert!(
        system[6].lightness < bg_l,
        "`max` ({:.3}) must be darker than the light page ({bg_l:.3})",
        system[6].lightness
    );
}

/// Every level survives the round trip through sRGB without clipping.
///
/// A level that leaves the gamut comes back desaturated, which silently breaks
/// the chroma ordering - so this is checked by construction and by measurement.
#[test]
fn every_level_is_inside_the_srgb_gamut() {
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        for background in [
            None,
            Some(RgbColor {
                r: 0x00,
                g: 0x2b,
                b: 0x36,
            }),
            Some(RgbColor {
                r: 0xfd,
                g: 0xf6,
                b: 0xe3,
            }),
        ] {
            let ladder = levels(background, scheme);
            for (index, level) in ladder.iter().enumerate() {
                // Round-tripping the colour we produced must give back the
                // lightness and chroma we asked for; a clipped value does not.
                let Color::Rgb(r, g, b) = level.color else {
                    panic!("a level is a concrete colour");
                };
                let (back_l, back_c, _) = oklch_of(RgbColor { r, g, b });
                assert!(
                    (back_l - level.lightness).abs() < 0.01,
                    "{scheme:?} level {index} (bg {background:?}): lightness drifted \
                     {:.4} -> {back_l:.4}, the colour clipped",
                    level.lightness
                );
                assert!(
                    (back_c - level.chroma).abs() < 0.02,
                    "{scheme:?} level {index} (bg {background:?}): chroma drifted \
                     {:.4} -> {back_c:.4}, the colour clipped",
                    level.chroma
                );
            }
        }
    }
}

/// The `system` palette's ladder is ordered too - the case the issue calls out
/// ("a ramp that only works on dark is the bug on half the terminals").
#[test]
fn the_system_palette_ramp_is_ordered_both_ways() {
    for (label, background, scheme) in [
        (
            "solarized dark",
            RgbColor {
                r: 0x00,
                g: 0x2b,
                b: 0x36,
            },
            ColorScheme::Dark,
        ),
        (
            "solarized light",
            RgbColor {
                r: 0xfd,
                g: 0xf6,
                b: 0xe3,
            },
            ColorScheme::Light,
        ),
        (
            "tokyo night",
            RgbColor {
                r: 0x1a,
                g: 0x1b,
                b: 0x26,
            },
            ColorScheme::Dark,
        ),
    ] {
        let palette = Palette::system(Some(background), scheme);
        let mut ladder = [Level {
            color: Color::Default,
            lightness: 0.0,
            chroma: 0.0,
            hue: 0.0,
        }; 7];
        for (slot, role) in ladder.iter_mut().zip(ROLES) {
            let Color::Rgb(r, g, b) = palette.get(role) else {
                panic!("{label}: {role:?} is a concrete colour");
            };
            let (lightness, chroma, hue) = oklch_of(RgbColor { r, g, b });
            *slot = Level {
                color: Color::Rgb(r, g, b),
                lightness,
                chroma,
                hue,
            };
        }
        let bg_l = crate::theme::panels::oklab_lightness_of(background);
        assert_ordered(label, scheme, &ladder, bg_l);
    }
}

/// No background reported: the levels still resolve, and still order.
///
/// The panels go transparent in this case (`Color::Default`); the border must
/// not, because a border with an empty colour is not a ramp at all.
#[test]
fn a_system_palette_without_a_probe_keeps_its_ladder() {
    let palette = Palette::system(None, ColorScheme::Dark);
    let mut seen: Vec<String> = Vec::new();
    for role in ROLES {
        let color = palette.get(role);
        assert_ne!(color, Color::Default, "{role:?} must still be a colour");
        seen.push(format!("{color:?}"));
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        seen.len(),
        7,
        "all seven levels stay distinct without a probe"
    );
    assert_ordered(
        "system/none",
        ColorScheme::Dark,
        &levels(None, ColorScheme::Dark),
        levels(None, ColorScheme::Dark)[0].lightness,
    );
}

/// NFR-28 in the plain theme: colour is gone, so the level must survive as
/// *text*.
///
/// The border itself carries no cue in plain mode - `Theme::plain()` maps every
/// role to the identity function, so the dashes are the same dashes at every
/// level. That is by design (NFR-28 is about the *interface*, not about every
/// widget), and the interface does satisfy it: the footer's stats segment prints
/// the level's name. This row pins that, because "the ramp is the only signal"
/// is exactly the accessibility regression the requirement exists to prevent.
#[test]
fn the_plain_theme_still_names_the_level_in_the_footer() {
    use crate::footer::Footer;

    let plain = crate::theme::Theme::plain();
    // The border is colourless at every level, so it cannot be the cue.
    let dashes: Vec<String> = [
        None,
        Some("off"),
        Some("minimal"),
        Some("low"),
        Some("medium"),
        Some("high"),
        Some("xhigh"),
        Some("max"),
    ]
    .iter()
    .map(|level| crate::separator::separator_border(&plain, *level)(&"─".repeat(8)))
    .collect();
    assert!(
        dashes.iter().all(|d| d == &dashes[0]),
        "the plain theme's border carries no cue: {dashes:?}"
    );

    // The footer does name the level, which is what makes NFR-28 hold.
    for level in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
        let footer = Footer {
            thinking: Some(level.to_string()),
            ..Footer::default()
        };
        let rendered = footer.render(100, &plain).join("\n");
        assert!(
            rendered.contains(level),
            "the plain footer must name the `{level}` level, got {rendered:?}"
        );
    }
}
