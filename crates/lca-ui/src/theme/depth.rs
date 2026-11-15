//! The 256-colour tier's lowering (gh #600): an RGB role becomes a palette
//! **index** once, at palette build.
//!
//! # Why a tier at all
//!
//! `ColorSupport` had three rungs and only two producers: `Truecolor` (a coloured
//! theme) and `None` (`ui.color = never`). `Ansi16` existed but nothing that ran in
//! a session ever produced it, and there was no **256** rung at all - so a
//! `TERM=xterm-256color` session with `COLORTERM` unset was handed 24-bit SGR it
//! cannot render, and the only honest answer a user had was `never`: no colour.
//! The detection that picks the rung lives in the engine
//! ([`lca_tui::engine::colors::color_depth`]); this module is the arithmetic that
//! spends it.
//!
//! # pi's three rules, quoted
//!
//! From `~/gits/pi/packages/tui/src/colors.ts`'s `rgbToAnsi256` (lines 279-300),
//! reproduced in integer arithmetic:
//!
//! 1. **The cube candidate** is nearest-per-channel over
//!    `[0, 95, 135, 175, 215, 255]`, at index `16 + 36*ri + 6*gi + bi`.
//! 2. **The ramp candidate** is the nearest of `8 + 10*i` (`i` in `0..24`, indices
//!    `232..=255`) to the luma-weighted grey `round(0.299r + 0.587g + 0.114b)`.
//! 3. **The ramp wins only when** `spread < 10` (spread = max channel − min channel)
//!    **and** its luma-weighted squared distance (`0.299dr² + 0.587dg² + 0.114db²`)
//!    is the smaller. Otherwise the cube.
//!
//! The weights are the same in both places pi uses them, so a colour that is
//! "grey enough" is judged by the same eye that chose its ramp entry.
//!
//! # The cost rule
//!
//! The lowering runs **once, at palette build** (this module is called from
//! `Theme::from_palette`), so the paint path is a captured `Color` and never
//! arithmetic. That is a claim no row can see - a `nearest_256` call inside a
//! `style()` closure would produce byte-identical frames while doing the work
//! again on every `format!` - so it is deliberate that the mechanism row
//! (`an_ansi256_theme_paints_indices_and_never_rgb`) asserts the **spelling**,
//! not a stopwatch, and the mutant that moves the lowering into `style()` is
//! reported as a **green** one (see the cycle's report).

use super::{Color, Palette, Role};

/// The cube's channel values (kitty/xterm's 6x6x6 cube).
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The grey ramp's values: `8 + 10*i`, `i` in `0..24`.
const RAMP: [u8; 24] = {
    let mut values = [0u8; 24];
    let mut i = 0;
    while i < 24 {
        values[i] = 8 + 10 * i as u8;
        i += 1;
    }
    values
};

/// The index of the entry in `values` nearest `target`, ties going to the lower
/// entry (pi's `findClosest`, which keeps the first minimum).
fn closest(values: &[u8], target: u8) -> usize {
    let mut best = 0usize;
    let mut best_distance = u16::MAX;
    for (index, value) in values.iter().enumerate() {
        let distance = target.abs_diff(*value);
        if u16::from(distance) < best_distance {
            best = index;
            best_distance = u16::from(distance);
        }
    }
    best
}

/// pi's luma-weighted squared distance (`colors.ts`'s `colorDistance`), in
/// integer arithmetic: the weights' common denominator is 1000, and the
/// comparison only needs the ordering, so a scale factor is free.
fn weighted_distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let dr = u32::from(a.0.abs_diff(b.0));
    let dg = u32::from(a.1.abs_diff(b.1));
    let db = u32::from(a.2.abs_diff(b.2));
    299 * dr * dr + 587 * dg * dg + 114 * db * db
}

/// The 256-colour index for one RGB triple, by pi's three rules (gh #600).
///
/// Returns `16..=255`: `16` is the cube's black, `231` its white, and
/// `232..=255` the grey ramp. The sixteen **system** colours are never chosen -
/// a role's 24-bit value is a hue, and the system palette is the terminal's own
/// (unrelated) pick, so snapping a themed colour to, say, "bright red" would
/// lose the theme's identity rather than preserve it.
pub fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    let (ri, gi, bi) = (closest(&CUBE, r), closest(&CUBE, g), closest(&CUBE, b));
    let cube = (CUBE[ri], CUBE[gi], CUBE[bi]);
    let cube_index = 16 + 36 * ri as u16 + 6 * gi as u16 + bi as u16;

    let luma = {
        // `round(0.299r + 0.587g + 0.114b)` over the same weights, in integers:
        // (299r + 587g + 114b + 500) / 1000 rounds half up, and the numerator's
        // maximum (99990) is far inside u32.
        let weighted = 299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b);
        ((weighted + 500) / 1000) as u8
    };
    let ramp_offset = closest(&RAMP, luma);
    let ramp_value = RAMP[ramp_offset];
    let ramp = (ramp_value, ramp_value, ramp_value);

    let spread = r.max(g).max(b) - r.min(g).min(b);
    if spread < 10 && weighted_distance((r, g, b), ramp) < weighted_distance((r, g, b), cube) {
        return 232 + ramp_offset as u8;
    }
    cube_index as u8
}

/// One colour as the current tier spells it.
///
/// A **theme-file** index is returned untouched at every depth: it is already a
/// palette entry the file's author chose, and re-deriving it would be the
/// second-place-decides defect (gh #600's byte-identity rule).
#[must_use]
pub fn lowered(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Ansi256(nearest_256(r, g, b)),
        other => other,
    }
}

/// The palette with every RGB role lowered to its 256-colour index.
///
/// Built once per theme (`Theme::from_palette`), then captured by every style
/// closure - which is what makes the paint path arithmetic-free. `Ansi256`
/// entries pass through, so a theme file that names an index is byte-identical.
#[must_use]
pub fn lowered_palette(palette: &Palette) -> Palette {
    let colors = Role::ALL
        .iter()
        .map(|role| (*role, lowered(palette.get(*role))))
        .collect();
    Palette::from_entries(colors)
}

#[cfg(test)]
mod depth_tests {
    use super::*;

    // Verifies: gh #600 - **both ends of the rule, because a gate that always
    // picks one is as wrong as the defect.** `#1e1e2e` is blue-tinted (spread 16)
    // so it takes the cube; `#808080` is neutral so it takes the ramp. The exact
    // indices are pi's own arithmetic (`colors.ts:279-300`), not eyeballed:
    // 30,30,46 → cube (0,0,0) = 16, and 128 greys to ramp entry 24 = 244.
    #[test]
    fn a_tinted_dark_lowers_to_the_cube_and_a_neutral_lowers_to_the_ramp() {
        assert_eq!(
            nearest_256(0x1e, 0x1e, 0x2e),
            16,
            "#1e1e2e -> the cube's black"
        );
        assert_eq!(
            nearest_256(0x80, 0x80, 0x80),
            244,
            "#808080 -> the grey ramp"
        );
        assert!(
            nearest_256(0x80, 0x80, 0x80) >= 232,
            "a neutral really is a ramp entry, not a cube one"
        );
        assert!(
            nearest_256(0x1e, 0x1e, 0x2e) < 232,
            "a tinted colour really is a cube entry"
        );
    }

    // Verifies: gh #600 - **the end stops and the `spread < 10` boundary.** The
    // cube's top stop is 231 and its bottom is 16 (the wire's own numbers), and
    // the boundary is exclusive on the left of it: a spread of 9 may take the
    // ramp, a spread of 10 may not. Both directions, as assertions.
    #[test]
    fn the_cube_and_ramp_end_stops_are_what_the_rule_says() {
        assert_eq!(nearest_256(255, 255, 255), 231, "the cube's white");
        assert_eq!(nearest_256(0, 0, 0), 16, "the cube's black");
        // Spread 9 (128,128,137): grey enough, so the ramp wins if it is closer.
        assert!(
            (232..=255).contains(&nearest_256(128, 128, 137)),
            "spread 9 may take the ramp: {}",
            nearest_256(128, 128, 137)
        );
        // Spread 10 (128,128,138): over the boundary, so the cube - 128 is a
        // cube *and* ramp stop, and the eleventh unit of blue is what decides.
        assert_eq!(
            nearest_256(128, 128, 138),
            102,
            "spread 10 may not: it takes the cube"
        );
    }

    // Verifies: gh #600 - **every built-in role lowers to a valid index.** The
    // cheap invariant that catches a stray value: both palettes, all 57 roles,
    // no panic and every result inside the cube-or-ramp range (never the
    // terminal's own system colours).
    #[test]
    fn every_built_in_role_lowers_without_panicking() {
        for palette in [Palette::dark(), Palette::light()] {
            let lowered = lowered_palette(&palette);
            for role in Role::ALL {
                match lowered.get(*role) {
                    Color::Ansi256(index) => assert!(
                        (16..=255).contains(&index),
                        "{} lowered to {index}, outside the cube and ramp",
                        role.key()
                    ),
                    Color::Default => {}
                    other => panic!("{} did not lower: {other:?}", role.key()),
                }
            }
        }
    }

    // Verifies: gh #600 - **a theme file's index is passed through untouched.** It
    // is already a palette entry the author chose; lowering it again (or leaving
    // it as a `Rgb`) would be the second-place-decides defect.
    #[test]
    fn a_theme_file_index_is_passed_through_untouched() {
        assert_eq!(lowered(Color::Ansi256(200)), Color::Ansi256(200));
        assert_eq!(lowered(Color::Ansi256(16)), Color::Ansi256(16));
        assert_eq!(lowered(Color::Default), Color::Default);
    }
}
#[cfg(test)]
mod theme_spelling_tests {
    use crate::theme::{ColorSupport, Role, Theme};

    // Verifies: gh #600 row 4 - **a built `Ansi256` theme paints indices and never
    // bytes.** The mechanism row: `fg()` carries `;5;N` and no `38;2`, `bg()`
    // carries `48;5;N` and no `48;2`, and the plain theme still emits nothing at
    // all. A stopwatch on a `format!` would be the flake class this tree has paid
    // for; the spelling is the observable.
    #[test]
    fn an_ansi256_theme_paints_indices_and_never_rgb() {
        let theme = Theme::from_palette_at(
            "palette",
            crate::theme::Palette::dark(),
            true,
            ColorSupport::Ansi256,
        );
        let fg = (theme.role(Role::Accent))("x");
        assert!(
            fg.contains("38;5;") && !fg.contains("38;2;"),
            "a 256-colour theme opens on the palette channel: {fg:?}"
        );
        assert!(
            fg.ends_with("\x1b[39m"),
            "and still closes only its own channel"
        );
        let bg = (theme.bg(Role::UserMessageBg))("x");
        assert!(
            bg.contains("48;5;") && !bg.contains("48;2;"),
            "the background lowers too: {bg:?}"
        );
        assert!(bg.ends_with("\x1b[49m"), "closing 49, never 0m");
        // The plain theme spends nothing at any depth.
        let plain = Theme::plain().at_support(ColorSupport::Ansi256);
        assert_eq!((plain.role(Role::Accent))("x"), "x");
        assert_eq!((plain.bg(Role::UserMessageBg))("x"), "x");
    }

    // Verifies: gh #600 row 5 - **a theme file's index stays byte-identical.** A
    // custom theme naming `Color::Ansi256(200)` is the author's palette choice, so
    // a 256-colour (and a truecolor) theme renders `;5;200` for that role exactly as
    // it did before this cycle: no lowering, no re-derivation. A sixteen-colour
    // terminal keeps its existing cap (`nearest_16` over the index's own RGB), which
    // is unchanged behaviour, not a new decision.
    #[test]
    fn a_theme_file_index_is_rendered_as_the_index_at_every_depth_that_can_spell_it() {
        let mut palette = crate::theme::Palette::dark();
        palette.set(Role::Accent, crate::theme::Color::Ansi256(200));
        for support in [ColorSupport::Truecolor, ColorSupport::Ansi256] {
            let theme = Theme::from_palette_at("file", palette.clone(), true, support);
            let spelled = (theme.role(Role::Accent))("x");
            assert!(
                spelled.contains("38;5;200"),
                "an index is passed through at {support:?}: {spelled:?}"
            );
            assert!(
                !spelled.contains("38;2;"),
                "and never re-derived into bytes: {spelled:?}"
            );
            // The styled-text path reads the same role, so the index reaches an
            // extension's span unchanged as well.
            let text = theme.style_text(
                "x",
                &lca_protocol::TextStyle {
                    fg: Some("accent".to_string()),
                    bg: None,
                    bold: false,
                    dim: false,
                    italic: false,
                    underline: false,
                },
            );
            assert!(
                text.contains("38;5;200"),
                "styled text keeps the index at {support:?}: {text:?}"
            );
        }
        // A sixteen-colour theme's **role** spelling is the uncapped `fg()` (only
        // the extension styled-text path consults the rung), so a file's index
        // reaches it as `;5;200` too - unchanged from before this cycle, and precisely
        // the behaviour the brief says must stay byte-identical.
        let sixteen = Theme::from_palette_at("file", palette.clone(), true, ColorSupport::Ansi16);
        assert!(
            (sixteen.role(Role::Accent))("x").contains("38;5;200"),
            "the role path never capped and still does not"
        );
    }
}
