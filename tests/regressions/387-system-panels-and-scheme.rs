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

// ---------------------------------------------------------------------------
// Gap 3: the built-in palettes are calibrated, not hand-picked
// ---------------------------------------------------------------------------

/// The dark palette's accent target: `OKHSL 67%` -> OKLab L 0.7161.
const DARK_TARGET_OKHSL: f64 = 0.67;
/// The light palette's accent target. **Not** pi's 47%: at 47% four of the five
/// accents land below the 4.5:1 body-text floor on LCA's own user band
/// (`#e8e8e8`) - success 3.85, warning 4.13, muted and dim 4.06. 41% is the
/// lightest value that clears the floor on every surface an accent is painted
/// on; the worst case there is 4.85:1.
const LIGHT_TARGET_OKHSL: f64 = 0.41;
/// How far an accent's OKLab lightness may sit from the target.
const TOLERANCE: f64 = 0.012;

/// Ottosson's toe inverse: OKHSL lightness -> OKLab L.
fn toe_inv(x: f64) -> f64 {
    let (k1, k2) = (0.206, 0.03);
    let k3 = (1.0 + k1) / (1.0 + k2);
    (x * x + k1 * x) / (k3 * (x + k2))
}

/// The accent family the calibration governs.
const ACCENTS: [lca_ui::theme::Role; 5] = [
    lca_ui::theme::Role::Success,
    lca_ui::theme::Role::Error,
    lca_ui::theme::Role::Warning,
    lca_ui::theme::Role::Accent,
    lca_ui::theme::Role::BorderAccent,
];

/// A role's OKLab lightness, measured from the shipped palette.
fn lightness_of(palette: &lca_ui::theme::Palette, role: lca_ui::theme::Role) -> f64 {
    lca_ui::theme::accents::role_lightness(palette, role)
        .unwrap_or_else(|| panic!("{} is a concrete colour", role.key()))
}

/// A role's RGB, from the shipped palette.
fn rgb_of(
    palette: &lca_ui::theme::Palette,
    role: lca_ui::theme::Role,
) -> lca_tui::engine::colors::RgbColor {
    match palette.get(role) {
        lca_ui::theme::Color::Rgb(r, g, b) => lca_tui::engine::colors::RgbColor { r, g, b },
        other => panic!("{} is not RGB: {other:?}", role.key()),
    }
}

/// **The calibration rule**, at the shipped palettes: every accent sits at one
/// perceptual lightness, so a warning and a success read at the same weight.
/// This is the row that makes the palette a rule rather than a repaint.
#[test]
fn every_accent_sits_at_its_palette_target_lightness() {
    for (label, palette, okhsl) in [
        ("dark", lca_ui::theme::Palette::dark(), DARK_TARGET_OKHSL),
        ("light", lca_ui::theme::Palette::light(), LIGHT_TARGET_OKHSL),
    ] {
        let target = toe_inv(okhsl);
        for role in ACCENTS {
            let measured = lightness_of(&palette, role);
            assert!(
                (measured - target).abs() <= TOLERANCE,
                "{label}: {} is OKLab L {measured:.4}, target {target:.4} \
                 (OKHSL {:.0}%)",
                role.key(),
                okhsl * 100.0
            );
        }
        // And the spread, which a per-role tolerance could hide.
        let lightnesses: Vec<f64> = ACCENTS.iter().map(|r| lightness_of(&palette, *r)).collect();
        let high = lightnesses.iter().cloned().fold(f64::MIN, f64::max);
        let low = lightnesses.iter().cloned().fold(f64::MAX, f64::min);
        assert!(
            high - low <= TOLERANCE,
            "{label}: the accent spread is {:.4}, wider than {TOLERANCE}",
            high - low
        );
    }
}

/// `muted` and `dim` keep the accent *weight* and lose the colour: same
/// lightness, near-zero chroma.
#[test]
fn the_neutrals_keep_the_weight_and_drop_the_colour() {
    for (label, palette, okhsl) in [
        ("dark", lca_ui::theme::Palette::dark(), DARK_TARGET_OKHSL),
        ("light", lca_ui::theme::Palette::light(), LIGHT_TARGET_OKHSL),
    ] {
        let target = toe_inv(okhsl);
        for role in [lca_ui::theme::Role::Muted, lca_ui::theme::Role::Dim] {
            let measured = lightness_of(&palette, role);
            let chroma = lca_ui::theme::accents::role_chroma(&palette, role)
                .unwrap_or_else(|| panic!("{} is concrete", role.key()));
            assert!(
                (measured - target).abs() <= TOLERANCE,
                "{label} {} is L {measured:.4}, target {target:.4}",
                role.key()
            );
            assert!(
                chroma <= 0.030,
                "{label} {} has chroma {chroma:.4} - it is a colour, not a \
                 de-emphasised accent weight",
                role.key()
            );
        }
    }
}

/// The issue's concrete complaint, by value: no built-in palette ships pure
/// `#ffff00`, and the `#8abeb7` teal is gone.
#[test]
fn the_reported_values_are_gone() {
    for (label, palette) in [
        ("dark", lca_ui::theme::Palette::dark()),
        ("light", lca_ui::theme::Palette::light()),
    ] {
        for role in lca_ui::theme::Role::ALL {
            if let lca_ui::theme::Color::Rgb(r, g, b) = palette.get(*role) {
                assert_ne!(
                    (r, g, b),
                    (0xff, 0xff, 0x00),
                    "{label} ships pure #ffff00 on {}",
                    role.key()
                );
                assert_ne!(
                    (r, g, b),
                    (0x8a, 0xbe, 0xb7),
                    "{label} still ships the teal on {}",
                    role.key()
                );
            }
        }
    }
}

/// **The contrast pairs asserted**, named: every accent, `muted`, `dim` and
/// `text`, against the four surfaces an accent or a de-emphasis is painted on -
/// the user band and the three tool-card states.
///
/// Deliberately **not** asserted: an accent on `selectedBg`. Nothing paints one
/// there (`render.rs` puts `SelectedBg` under `Role::Text` only; `chat_render.rs`
/// selects `Role::Text`), and neither pi's palette nor this one clears 4.5:1 on a
/// selected row (pi's own figures are 3.76-4.55). Asserting it would assert a
/// requirement the product does not have.
#[test]
fn the_de_emphasis_and_accents_are_readable_where_they_are_painted() {
    const FLOOR: f64 = 4.5;
    let surfaces = [
        lca_ui::theme::Role::UserMessageBg,
        lca_ui::theme::Role::ToolSuccessBg,
        lca_ui::theme::Role::ToolErrorBg,
        lca_ui::theme::Role::ToolPendingBg,
    ];
    for (label, palette) in [
        ("dark", lca_ui::theme::Palette::dark()),
        ("light", lca_ui::theme::Palette::light()),
    ] {
        let foregrounds: Vec<lca_ui::theme::Role> = ACCENTS
            .iter()
            .copied()
            .chain([
                lca_ui::theme::Role::Muted,
                lca_ui::theme::Role::Dim,
                lca_ui::theme::Role::Text,
            ])
            .collect();
        for role in foregrounds {
            let foreground = rgb_of(&palette, role);
            for surface in surfaces {
                let background = rgb_of(&palette, surface);
                let ratio = lca_ui::theme::accents::contrast(foreground, background);
                assert!(
                    ratio >= FLOOR,
                    "{label}: {} on {} is {ratio:.2}:1, below {FLOOR}:1",
                    role.key(),
                    surface.key()
                );
            }
        }
    }
}

/// The shipped values are what the *rule* produces, so editing a hex without
/// editing the rule (or the reverse) fails here rather than drifting.
#[test]
fn the_shipped_palette_is_the_rules_output() {
    use lca_ui::theme::accents::{accent, hex, neutral};
    let authored = [
        (lca_ui::theme::Role::Accent, 295.0_f64, 0.85_f64),
        (lca_ui::theme::Role::BorderAccent, 295.0, 0.85),
        (lca_ui::theme::Role::Success, 159.0, 0.85),
        (lca_ui::theme::Role::Error, 20.0, 0.85),
        (lca_ui::theme::Role::Warning, 83.0, 0.85),
    ];
    for (label, palette, okhsl) in [
        ("dark", lca_ui::theme::Palette::dark(), DARK_TARGET_OKHSL),
        ("light", lca_ui::theme::Palette::light(), LIGHT_TARGET_OKHSL),
    ] {
        for (role, hue, fraction) in authored {
            assert_eq!(
                rgb_of(&palette, role),
                accent(okhsl, hue, fraction),
                "{label} {} is {} but the rule produces {}",
                role.key(),
                hex(rgb_of(&palette, role)),
                hex(accent(okhsl, hue, fraction))
            );
        }
        for (role, chroma) in [
            (lca_ui::theme::Role::Muted, 0.012),
            (lca_ui::theme::Role::Dim, 0.020),
        ] {
            assert_eq!(
                rgb_of(&palette, role),
                neutral(okhsl, chroma),
                "{label} {} is not what the rule produces",
                role.key()
            );
        }
    }
}

/// `plain` emits no SGR, so no palette value can reach it.
#[test]
fn the_plain_theme_is_untouched_by_the_palette() {
    let theme = lca_ui::theme::Theme::plain();
    for role in ACCENTS
        .iter()
        .copied()
        .chain([lca_ui::theme::Role::Muted, lca_ui::theme::Role::Dim])
    {
        let painted = (theme.role(role))("x");
        assert!(
            !painted.contains('\x1b'),
            "plain {} emitted an escape: {painted:?}",
            role.key()
        );
    }
}

/// A user's theme file still wins over every calibrated value, and a role it
/// does not name keeps the palette's own value - so an existing theme is
/// unaffected by this cycle.
#[test]
fn a_user_theme_file_still_wins_over_the_calibration() {
    let base = lca_ui::theme::Palette::dark();
    let (overlaid, applied) = base
        .overlay("[colors]\nwarning = \"#123456\"\n")
        .expect("a valid theme file");
    assert_eq!(applied, 1);
    assert_eq!(
        overlaid.get(lca_ui::theme::Role::Warning),
        lca_ui::theme::Color::Rgb(0x12, 0x34, 0x56)
    );
    assert_eq!(
        overlaid.get(lca_ui::theme::Role::Success),
        base.get(lca_ui::theme::Role::Success),
        "a role the file does not name keeps the palette's value"
    );
}
