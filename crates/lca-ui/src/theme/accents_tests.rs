//! Gh #387 gap 3: the calibrated built-in palettes.
//!
//! The rule is [`super::accents`]; this file pins it against the palettes that
//! actually ship, so a future hand-tweak of one hex value fails here.

use crate::theme::accents::{
    ACCENT_ROLES, DARK_ACCENT_OKHSL, LIGHT_ACCENT_OKHSL, LIGHTNESS_TOLERANCE, NEUTRAL_CHROMA_MAX,
    accent, contrast, hex, neutral, okhsl_to_oklab_lightness, relative_luminance, role_chroma,
    role_lightness,
};
use crate::theme::{Color, Palette, Role};

/// The RGB behind a role, or a panic naming it (every role here is an RGB value).
fn rgb(palette: &Palette, role: Role) -> lca_tui::engine::colors::RgbColor {
    match palette.get(role) {
        Color::Rgb(r, g, b) => lca_tui::engine::colors::RgbColor { r, g, b },
        other => panic!("{} is not an RGB colour: {other:?}", role.key()),
    }
}

// ---------------------------------------------------------------------------
// The calibration rule (the deliverable)
// ---------------------------------------------------------------------------

// Every accent role sits at the palette's target lightness. This is the row that
// makes the palette a *rule* rather than a repaint: hand-tweaking one hue's
// brightness to taste now fails here.
#[test]
fn every_dark_accent_sits_at_the_target_lightness() {
    let palette = Palette::dark();
    let target = okhsl_to_oklab_lightness(DARK_ACCENT_OKHSL);
    for role in ACCENT_ROLES {
        let lightness = role_lightness(&palette, role).expect("an RGB accent");
        assert!(
            (lightness - target).abs() <= LIGHTNESS_TOLERANCE,
            "{} is OKLab L {lightness:.4}, target {target:.4} (OKHSL {:.0}%)",
            role.key(),
            DARK_ACCENT_OKHSL * 100.0
        );
    }
}

#[test]
fn every_light_accent_sits_at_the_target_lightness() {
    let palette = Palette::light();
    let target = okhsl_to_oklab_lightness(LIGHT_ACCENT_OKHSL);
    for role in ACCENT_ROLES {
        let lightness = role_lightness(&palette, role).expect("an RGB accent");
        assert!(
            (lightness - target).abs() <= LIGHTNESS_TOLERANCE,
            "{} is OKLab L {lightness:.4}, target {target:.4} (OKHSL {:.0}%)",
            role.key(),
            LIGHT_ACCENT_OKHSL * 100.0
        );
    }
}

// The row above would still pass if the tolerance were satisfied by a *wide*
// spread, so this one states the property the issue is about directly: the
// **spread** across accents is small. A hand-tweak that moved one accent by the
// whole tolerance in one direction and another by the whole tolerance in the
// other would slip past the per-role check by construction; it cannot slip past
// this one.
#[test]
fn the_accents_are_within_a_narrow_spread_of_each_other() {
    for (label, palette) in [("dark", Palette::dark()), ("light", Palette::light())] {
        let lightnesses: Vec<f64> = ACCENT_ROLES
            .iter()
            .filter_map(|role| role_lightness(&palette, *role))
            .collect();
        let high = lightnesses.iter().cloned().fold(f64::MIN, f64::max);
        let low = lightnesses.iter().cloned().fold(f64::MAX, f64::min);
        assert!(
            (high - low) <= LIGHTNESS_TOLERANCE,
            "{label}: the accent lightness spread is {:.4}, wider than {LIGHTNESS_TOLERANCE}",
            high - low
        );
    }
}

// Weight preserved, colour removed: `muted` and `dim` share the accent
// lightness at near-zero chroma. That is what makes a hint the same *size* as a
// message while carrying no emphasis.
#[test]
fn muted_and_dim_hold_the_accent_lightness_without_the_colour() {
    for (label, palette, okhsl) in [
        ("dark", Palette::dark(), DARK_ACCENT_OKHSL),
        ("light", Palette::light(), LIGHT_ACCENT_OKHSL),
    ] {
        let target = okhsl_to_oklab_lightness(okhsl);
        for role in [Role::Muted, Role::Dim] {
            let lightness = role_lightness(&palette, role).expect("an RGB neutral");
            let chroma = role_chroma(&palette, role).expect("an RGB neutral");
            assert!(
                (lightness - target).abs() <= LIGHTNESS_TOLERANCE,
                "{label} {} is OKLab L {lightness:.4}, target {target:.4}",
                role.key()
            );
            assert!(
                chroma <= NEUTRAL_CHROMA_MAX,
                "{label} {} has chroma {chroma:.4}, above {NEUTRAL_CHROMA_MAX} - it is \
                 a colour, not a de-emphasis of the accent weight",
                role.key()
            );
        }
    }
}

// `dim` is the *second* de-emphasis level, so it must not be the same value as
// `muted` - otherwise the third level does not exist and the vocabulary lies.
#[test]
fn dim_is_a_distinct_third_level_from_muted() {
    for (label, palette) in [("dark", Palette::dark()), ("light", Palette::light())] {
        assert_ne!(
            rgb(&palette, Role::Muted),
            rgb(&palette, Role::Dim),
            "{label}: `dim` is a separate level from `muted`"
        );
    }
}

// ---------------------------------------------------------------------------
// The concrete complaints from the issue
// ---------------------------------------------------------------------------

// Pure `#ffff00` is the value the issue names. It is gone from both palettes,
// asserted *by value* rather than by "no longer yellow", because the complaint
// was literally the hex.
#[test]
fn no_builtin_palette_ships_pure_yellow() {
    let pure = lca_tui::engine::colors::RgbColor {
        r: 0xff,
        g: 0xff,
        b: 0x00,
    };
    for (label, palette) in [("dark", Palette::dark()), ("light", Palette::light())] {
        for role in Role::ALL {
            if let Color::Rgb(r, g, b) = palette.get(*role) {
                assert_ne!(
                    (r, g, b),
                    (pure.r, pure.g, pure.b),
                    "{label} ships pure #ffff00 on {}",
                    role.key()
                );
            }
        }
    }
}

// The teal `#8abeb7` that stood in for the accent family is gone too.
#[test]
fn the_old_teal_is_gone() {
    let teal = lca_tui::engine::colors::RgbColor {
        r: 0x8a,
        g: 0xbe,
        b: 0xb7,
    };
    for (label, palette) in [("dark", Palette::dark()), ("light", Palette::light())] {
        for role in Role::ALL {
            if let Color::Rgb(r, g, b) = palette.get(*role) {
                assert_ne!(
                    (r, g, b),
                    (teal.r, teal.g, teal.b),
                    "{label} still ships the teal on {}",
                    role.key()
                );
            }
        }
    }
}

// The warning is no longer a saturated primary, and it is still distinguishable
// from success and error by hue - the point of keeping per-hue chroma rather
// than one flat value.
#[test]
fn the_accents_are_distinguishable_by_hue() {
    for (label, palette) in [("dark", Palette::dark()), ("light", Palette::light())] {
        let mut hues: Vec<(Role, f64)> = ACCENT_ROLES
            .iter()
            .filter_map(|role| match palette.get(*role) {
                Color::Rgb(r, g, b) => Some((
                    *role,
                    crate::theme::panels::oklch_of(lca_tui::engine::colors::RgbColor { r, g, b }).2,
                )),
                _ => None,
            })
            .collect();
        hues.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        // `accent` and `borderAccent` share a hue by design (the border is the
        // accent), so the distinct-hue set is four, not five.
        let distinct: Vec<f64> = hues
            .iter()
            .map(|(_, hue)| *hue)
            .collect::<Vec<f64>>()
            .windows(2)
            .fold(vec![hues[0].1], |mut acc, pair| {
                if (pair[1] - pair[0]).abs() > 8.0 {
                    acc.push(pair[1]);
                }
                acc
            });
        assert!(
            distinct.len() >= 4,
            "{label}: the accents resolve to {} distinct hues ({distinct:?}), \
             so a warning cannot be told from a success by colour",
            distinct.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Contrast (de-emphasis that cannot be read is a bug)
// ---------------------------------------------------------------------------

/// The contrast floor for body text (WCAG AA, normal size).
const BODY_TEXT_FLOOR: f64 = 4.5;

// **The pairs asserted, and why only these.**
//
// An accent is painted on the *page* and inside modal frames; it is never
// painted onto `selectedBg` or a tool card (checked: `render.rs` uses
// `SelectedBg` only under `Role::Text`, and `chat_render.rs` selects
// `Role::Text` on it). So the honest pairs are accent-on-panel for the four
// panel surfaces, and `text`/`muted`/`dim` on those same surfaces - which is
// where de-emphasis actually has to stay readable.
//
// `selectedBg` is **excluded** on purpose: neither pi's palette nor this one
// clears 4.5:1 for a light accent on a selected row (pi's own figures are
// 3.76-4.55), and nothing paints an accent there. Asserting it would be
// asserting a requirement the product does not have, and the honest thing is to
// say so rather than to smuggle in a threshold nobody meets.
#[test]
fn the_accents_clear_the_floor_on_the_reading_surfaces() {
    for (label, palette, surfaces) in [
        (
            "dark",
            Palette::dark(),
            [
                Role::UserMessageBg,
                Role::ToolSuccessBg,
                Role::ToolErrorBg,
                Role::ToolPendingBg,
            ],
        ),
        (
            "light",
            Palette::light(),
            [
                Role::UserMessageBg,
                Role::ToolSuccessBg,
                Role::ToolErrorBg,
                Role::ToolPendingBg,
            ],
        ),
    ] {
        for role in ACCENT_ROLES {
            let accent = rgb(&palette, role);
            for surface in surfaces {
                let background = rgb(&palette, surface);
                let ratio = contrast(accent, background);
                assert!(
                    ratio >= BODY_TEXT_FLOOR,
                    "{label}: {} on {} is {ratio:.2}:1, below the {BODY_TEXT_FLOOR}:1 floor",
                    role.key(),
                    surface.key()
                );
            }
        }
    }
}

// The de-emphasis roles are where readability is easiest to lose: `dim` is
// deliberately low-contrast, and it is still body text.
#[test]
fn the_de_emphasis_roles_stay_readable() {
    for (label, palette, floor) in [
        // pi's own `dim` is a hint/footer role; 4.5:1 is the body-text floor and
        // these roles clear it, so there is no reason to settle for less.
        ("dark", Palette::dark(), BODY_TEXT_FLOOR),
        ("light", Palette::light(), BODY_TEXT_FLOOR),
    ] {
        for role in [Role::Muted, Role::Dim, Role::Text] {
            let foreground = rgb(&palette, role);
            for surface in [
                Role::UserMessageBg,
                Role::ToolSuccessBg,
                Role::ToolErrorBg,
                Role::ToolPendingBg,
            ] {
                let background = rgb(&palette, surface);
                let ratio = contrast(foreground, background);
                assert!(
                    ratio >= floor,
                    "{label}: {} on {} is {ratio:.2}:1, below {floor}:1",
                    role.key(),
                    surface.key()
                );
            }
        }
    }
}

// The rule is *computed*, not guessed: the shipped value for each accent equals
// what the rule produces. This is the row that stops the table drifting back
// into hand-picked hex - if someone edits a value without editing the rule, or
// the reverse, they disagree here.
#[test]
fn the_shipped_values_are_what_the_rule_produces() {
    // Hue and chroma fraction per accent, as the palettes author them.
    let authored = [
        (Role::Accent, 295.0_f64, 0.85_f64),
        (Role::BorderAccent, 295.0, 0.85),
        (Role::Success, 159.0, 0.85),
        (Role::Error, 20.0, 0.85),
        (Role::Warning, 83.0, 0.85),
    ];
    for (label, palette, okhsl) in [
        ("dark", Palette::dark(), DARK_ACCENT_OKHSL),
        ("light", Palette::light(), LIGHT_ACCENT_OKHSL),
    ] {
        for (role, hue, fraction) in authored {
            let expected = accent(okhsl, hue, fraction);
            assert_eq!(
                rgb(&palette, role),
                expected,
                "{label} {} is {} but the rule produces {}",
                role.key(),
                hex(rgb(&palette, role)),
                hex(expected)
            );
        }
        for (role, chroma) in [(Role::Muted, 0.012), (Role::Dim, 0.020)] {
            let expected = neutral(okhsl, chroma);
            assert_eq!(
                rgb(&palette, role),
                expected,
                "{label} {} is {} but the rule produces {}",
                role.key(),
                hex(rgb(&palette, role)),
                hex(expected)
            );
        }
    }
}

// The measurement helpers themselves: if `okhsl_to_oklab_lightness` were wrong,
// every row above would agree with a wrong target.
#[test]
fn the_lightness_conversion_matches_the_okhsl_definition() {
    // The toe inverse is monotone and anchored at 0 and 1.
    assert!((okhsl_to_oklab_lightness(0.0) - 0.0).abs() < 1e-9);
    assert!((okhsl_to_oklab_lightness(1.0) - 1.0).abs() < 1e-9);
    // A known point: OKHSL 50% is very close to OKLab 0.5 by construction of the
    // toe (it is not exactly equal, which is the whole reason the conversion
    // exists), and it must be between its neighbours.
    let a = okhsl_to_oklab_lightness(0.4);
    let b = okhsl_to_oklab_lightness(0.5);
    let c = okhsl_to_oklab_lightness(0.6);
    assert!(a < b && b < c, "monotone: {a} < {b} < {c}");
    // And a rendered colour at that lightness *measures* at it, which is the
    // property the palettes rely on.
    let target = okhsl_to_oklab_lightness(0.67);
    let measured = crate::theme::panels::oklab_lightness(accent(0.67, 295.0, 0.85));
    assert!(
        (measured - target).abs() < LIGHTNESS_TOLERANCE,
        "a colour authored at {target:.4} measures {measured:.4}"
    );
}

// White on black is the contrast scale's own anchor; if this is wrong every
// ratio above is.
#[test]
fn the_contrast_helper_is_the_wcag_scale() {
    let white = lca_tui::engine::colors::RgbColor {
        r: 0xff,
        g: 0xff,
        b: 0xff,
    };
    let black = lca_tui::engine::colors::RgbColor { r: 0, g: 0, b: 0 };
    assert!((relative_luminance(black) - 0.0).abs() < 1e-9);
    assert!((relative_luminance(white) - 1.0).abs() < 1e-6);
    assert!((contrast(white, black) - 21.0).abs() < 0.01);
    assert!((contrast(white, white) - 1.0).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// No regressions in the surface behaviour this cycle must not disturb
// ---------------------------------------------------------------------------

// `plain` emits no SGR, so the palette is irrelevant to it.
#[test]
fn the_plain_theme_still_emits_no_sgr() {
    let themed = crate::theme::Theme::plain();
    for role in [
        Role::Success,
        Role::Warning,
        Role::Error,
        Role::Muted,
        Role::Dim,
    ] {
        let styled = (themed.role(role))("x");
        assert!(
            !styled.contains('\x1b'),
            "plain {} emitted an escape: {styled:?}",
            role.key()
        );
    }
}

// The `system` theme still derives from the detected palette rather than
// hardcoding, and it picks its base by scheme.
#[test]
fn the_system_theme_still_derives_by_scheme() {
    for scheme in [
        lca_tui::engine::colors::ColorScheme::Dark,
        lca_tui::engine::colors::ColorScheme::Light,
    ] {
        let system = Palette::system(None, scheme);
        let base = match scheme {
            lca_tui::engine::colors::ColorScheme::Dark => Palette::dark(),
            lca_tui::engine::colors::ColorScheme::Light => Palette::light(),
        };
        for role in ACCENT_ROLES {
            assert_eq!(
                system.get(role),
                base.get(role),
                "{scheme:?} system {} follows its scheme base",
                role.key()
            );
        }
    }
}

// A user's theme file is untouched: it wins over every calibrated value, for
// every role it names, and the palettes' own values are what it does *not* name.
#[test]
fn a_user_theme_file_still_wins() {
    let base = Palette::dark();
    let (overlaid, applied) = base
        .overlay("[colors]\nwarning = \"#123456\"\n")
        .expect("a valid theme file");
    assert_eq!(applied, 1);
    assert_eq!(
        overlaid.get(Role::Warning),
        Color::Rgb(0x12, 0x34, 0x56),
        "the user's warning wins over the calibrated one"
    );
    // A role the file did not name keeps the calibrated palette's value, so an
    // existing theme file does not silently change how everything else renders.
    assert_eq!(
        overlaid.get(Role::Success),
        base.get(Role::Success),
        "an unnamed role keeps the palette's own value"
    );
}

// The gap-5 fallback table still resolves every role: this cycle changes values,
// not the vocabulary.
#[test]
fn every_role_still_resolves_through_its_chain() {
    for (label, palette) in [
        ("dark", Palette::dark()),
        ("light", Palette::light()),
        (
            "system-dark",
            Palette::system(None, lca_tui::engine::colors::ColorScheme::Dark),
        ),
        (
            "system-light",
            Palette::system(None, lca_tui::engine::colors::ColorScheme::Light),
        ),
    ] {
        assert!(palette.is_complete(), "{label} names every role");
        // And the table is sound, which the load path validates.
        crate::theme::tokens::validate().expect("the fallback table is sound");
    }
}
