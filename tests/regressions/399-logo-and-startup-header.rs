//! gh #399 — the startup logo and header: NFR-24's guard.
//!
//! The brief treats the owner's logo as a **supplied asset**, not a design
//! question, so the risk this file guards is not "the header looks wrong" but the
//! three ways a supplied asset silently stops being supplied:
//!
//! 1. **The grid gets prettified.** A thicker bottom stroke, a rounded corner, a
//!    re-centred baseline is a *different logo*, and it would pass any test that
//!    only asserts "something rendered". The row below pins the exact three rows
//!    the owner's five-row sketch pairs into.
//! 2. **The logo colours drift into the theme.** The owner was explicit that they
//!    are icon colours, not the app theme. The guard is that the three hex values
//!    appear in no built-in palette, so nobody can later "just" alias a role to
//!    them - and the behaviour rows prove a `/theme` switch leaves the mark alone.
//! 3. **The fallback becomes a guess.** Apple_Terminal's half-block bug is real,
//!    and the rule is the *pair* pi measured (`platform == darwin` **and**
//!    `TERM_PROGRAM == Apple_Terminal`), not either half alone.
//!
//! Behaviour rows live beside their modules:
//! `crates/lca-ui/src/logo_tests.rs` (11 rows), `startup_header_tests.rs` (8) and
//! `startup_header_wired_tests.rs` (5, the quiet ladder and headless);
//! `crates/lca-tui/src/engine/terminal_tests.rs` (2, the detection rule).

use std::path::Path;

fn read(path: &str) -> String {
    // `CARGO_MANIFEST_DIR` is the workspace root here: the regression crate lives at
    // `tests/regressions` and its manifest points at the root, like #410's reader.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

/// The owner's grid, verbatim from `owner_notes.md`.
const OWNER_GRID: [&str; 5] = [
    "l.ccc.aa.",
    "l.c...a.a",
    "l.c...aaa",
    "l.ccc.a.a",
    "lllllllll",
];

// Verifies: NFR-24 - the logo is drawn from the owner's grid, and the grid in the
// source is the grid the owner wrote.
//
// The behavioural row (`the_mark_is_the_owner_grid_half_blocks`) asserts the
// rendered rows; this one asserts the **input** is unedited. Both are needed: a
// renderer bug and a prettified source produce the same wrong picture, and only
// comparing the source to the owner's sketch says which happened.
#[test]
fn the_source_grid_is_the_owners_five_rows() {
    let logo = read("crates/lca-ui/src/logo.rs");
    for row in OWNER_GRID {
        assert!(
            logo.contains(&format!("\"{row}\"")),
            "the owner's grid row {row:?} must appear verbatim in logo.rs - a \
             prettified variant is a different logo"
        );
    }
    // And the grid is the only thing that produces the glyphs: no row of block
    // characters is written out by hand, which would let the two drift.
    for hand_drawn in ["\"█ █▀▀ █▀▄\"", "\"█ █▄▄ █▀█\"", "\"▀▀▀▀▀▀▀▀▀\""]
    {
        assert!(
            !logo.contains(hand_drawn),
            "the rendered row {hand_drawn} must be *derived* from the grid, not typed \
             out - typing it is how a grid edit stops reaching the screen"
        );
    }
}

// Verifies: NFR-24 - the three colours are the owner's, exactly, and none of them
// is a theme role.
//
// The second half is the whole owner constraint. "The colours must not respond to
// `/theme`" is only true by construction if no palette can name them, so the guard
// is that the values are absent from every built-in palette rather than that the
// renderer happens not to read one.
#[test]
fn the_logo_colours_are_the_owners_and_are_not_theme_roles() {
    let logo = read("crates/lca-ui/src/logo.rs");
    for rgb in ["0x62, 0xae, 0xdc", "0xe2, 0x8c, 0x00", "0xec, 0xcd, 0x00"] {
        assert!(
            logo.contains(rgb),
            "the owner's colour ({rgb}) must appear in the logo's own table"
        );
    }
    // And no palette file mentions any of the three hex strings.
    for palette in [
        "crates/lca-ui/src/theme/palette.rs",
        "crates/lca-ui/src/theme/tokens.rs",
    ] {
        let source = read(palette).to_lowercase();
        for hex in ["62aedc", "e28c00", "eccd00"] {
            assert!(
                !source.contains(hex),
                "{palette} must not contain the logo colour #{hex}: the logo's colours \
                 are icon colours, not theme roles"
            );
        }
    }
}

// Verifies: NFR-24 - Apple_Terminal is detected by the pair, not by either half.
//
// A Linux terminal can export Apple's name and a Mac can be running something
// else; half blocks render fine on the first, and only Apple's Terminal has the
// gap bug. This pins that both conditions are in the expression, so a future
// "simplification" to `TERM_PROGRAM == Apple_Terminal` fails here.
#[test]
fn the_apple_terminal_rule_needs_the_platform_and_the_program() {
    let terminal = read("crates/lca-tui/src/engine/terminal.rs");
    let start = terminal
        .find("fn detect_apple_terminal(")
        .expect("the rule exists");
    let body = &terminal[start..];
    let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
    // The *final expression* is what decides, so this reads the last line rather
    // than the whole body: `is_macos` also appears in the parameter list and in the
    // forcing branch, so a `body.contains` check passes even after the platform half
    // is deleted from the rule - which is how the first version of this row missed
    // exactly that mutation.
    let rule = body
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("is_macos ") || line.starts_with("env("))
        .unwrap_or("");
    assert!(
        rule.contains("is_macos &&"),
        "the deciding expression is the platform AND the program name, pi's pair;          the platform half is not optional: {rule:?}"
    );
    assert!(
        rule.contains("Apple_Terminal"),
        "and it still names the terminal it is about: {rule:?}"
    );
    // The forcing hook is documented, because the fallback has to be demonstrable
    // on a machine that is not a Mac.
    assert!(
        terminal.contains("LCA_HALF_BLOCKS"),
        "the fallback is forceable for a receipt"
    );
}

// Verifies: NFR-24 - the header's keys are never spelled out.
//
// gh #410's lesson applied to the first thing a user reads: a hard-coded `ctrl+c`
// is a hint that can lie after a rebind. Every key on the line must come through
// the live manager, so the guard is that the module contains no key literal at all.
#[test]
fn no_key_on_the_header_is_hardcoded() {
    let header = read("crates/lca-ui/src/startup_header.rs");
    let start = header
        .find("pub fn hints(")
        .expect("the hint builder exists");
    let body = &header[start..];
    let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
    for key in ["ctrl+c", "ctrl+d", "escape", "alt+", "ctrl+o", "ctrl+q"] {
        assert!(
            !body.contains(&format!("\"{key}")),
            "the hint builder must not spell {key:?}: it comes from the live \
             registry, or an unbound action would still advertise a key"
        );
    }
    assert!(
        body.matches("display_keys(").count() >= 3,
        "every action-backed hint resolves through the live manager: {body}"
    );
}

// Verifies: NFR-24 - `quietStartup` still wins over the logo.
//
// The setting predates the logo and worker 2 holds the startup module this window,
// so the one thing that must not regress is that a quiet startup still shows
// nothing. A logo that ignored it would be a settings bug, not a cosmetic one.
#[test]
fn a_quiet_startup_still_shows_no_logo() {
    let chat = read("crates/lca-ui/src/chat.rs");
    assert!(
        chat.contains("push_startup_header()"),
        "the header entry is pushed unconditionally, so the *renderer* decides the rung"
    );
    let render = read("crates/lca-ui/src/transcript/render.rs");
    assert!(
        render.contains("StartupHeaderRung::Hidden => Vec::new()"),
        "the hidden rung draws no rows at all"
    );
    // And the host maps all three spellings, so none of them can fall through.
    let hooks = read("crates/lca-cli/src/tui/hooks.rs");
    for rung in ["Hidden", "VersionOnly", "Full"] {
        assert!(
            hooks.contains(rung),
            "the host maps quietStartup onto {rung}"
        );
    }
}
