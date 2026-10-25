//! Gh #387 gap 3 in a real terminal (tmux): the calibrated palettes.
//!
//! The unit rows prove the rule; this row proves the **bytes a terminal
//! receives** with each palette, so the calibrated relationships are on the
//! record and not just in a table.
//!
//! For each of `/theme dark` and `/theme light` it captures a frame
//! (`capture-pane -e`) and asserts the accent family's SGR sequence is the
//! calibrated value - and, crucially, that the frame carries **one** accent
//! weight rather than the old mixture of a pure-yellow warning and a teal
//! accent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
mod common;

#[cfg(unix)]
use common::*;

/// The SGR parameter string a role paints with, from the real palette.
#[cfg(unix)]
fn sgr_for(theme: &lca_ui::theme::Theme, role: lca_ui::theme::Role) -> String {
    let painted = (theme.role(role))("x");
    painted.split('x').next().unwrap_or_default().to_string()
}

// Verifies: gh #387 gap 3 - in a real pane, the dark and light palettes paint
// their calibrated accent values, and the old pure-yellow and teal are absent.
#[cfg(unix)]
#[test]
fn the_calibrated_palettes_reach_the_terminal() {
    if !tmux_available() {
        eprintln!("skip: tmux is not installed (real-terminal rows are Unix-only)");
        return;
    }
    let runtime = rt();
    let mock = runtime.block_on(start_mock(vec![Reply::Sse(sse_text("unused"))]));
    let sandbox = sandbox("theme-387");
    sandbox.approve_loopback_net(serde_json::json!({}));

    let session = Tmux::new("theme-387");
    session.spawn(&sandbox, Some(&mock), true, &[], &[]);
    session.wait_for("[session in", std::time::Duration::from_secs(20));
    session.wait_for_input_ready(std::time::Duration::from_secs(15));

    // The old values this gap exists to remove.
    const OLD_YELLOW: &str = "\x1b[38;2;255;255;0m";
    const OLD_TEAL: &str = "\x1b[38;2;138;190;183m";

    // **`/theme` opens a picker and ignores its argument** - verified by reading
    // `chat_commands.rs`, and by this row failing silently the first time it was
    // written with `/theme dark\n`. The picker's order is `THEMES`
    // (`dark`, `light`, `plain`, `system`) and it opens on the *current* theme,
    // so selecting a scheme is: `/theme`, then move to the row, then Enter.
    for (scheme, palette, model, moves) in [
        (
            "dark",
            lca_ui::theme::Palette::dark(),
            lca_ui::theme::Theme::colored(),
            0usize,
        ),
        (
            "light",
            lca_ui::theme::Palette::light(),
            lca_ui::theme::Theme::light(),
            1usize,
        ),
    ] {
        session.send(&["C-u"]);
        session.send(&["/theme", "Enter"]);
        // The picker lists the built-ins; wait for the row that is the target.
        session.wait_for(scheme, std::time::Duration::from_secs(20));
        for _ in 0..moves {
            session.send(&["Down"]);
        }
        session.send(&["Enter"]);
        let pane = session.wait_until_painted(std::time::Duration::from_secs(10));

        // `-e` keeps the SGR, which is what this row is about.
        let with_escapes = session.capture_e();

        for role in [
            lca_ui::theme::Role::Warning,
            lca_ui::theme::Role::Accent,
            lca_ui::theme::Role::Success,
            lca_ui::theme::Role::Error,
            lca_ui::theme::Role::Muted,
            lca_ui::theme::Role::Dim,
        ] {
            let sgr = sgr_for(&model, role);
            assert!(
                !sgr.is_empty(),
                "{scheme}: {} paints a colour, not the terminal default",
                role.key()
            );
        }

        // The calibration, read off the palette the pane is using.
        let target = lca_ui::theme::accents::okhsl_to_oklab_lightness(if scheme == "dark" {
            lca_ui::theme::accents::DARK_ACCENT_OKHSL
        } else {
            lca_ui::theme::accents::LIGHT_ACCENT_OKHSL
        });
        for role in [
            lca_ui::theme::Role::Warning,
            lca_ui::theme::Role::Accent,
            lca_ui::theme::Role::Success,
            lca_ui::theme::Role::Error,
        ] {
            let measured = lca_ui::theme::accents::role_lightness(&palette, role)
                .unwrap_or_else(|| panic!("{scheme}: {} is concrete", role.key()));
            assert!(
                (measured - target).abs() <= lca_ui::theme::accents::LIGHTNESS_TOLERANCE,
                "{scheme}: {} is L {measured:.4}, target {target:.4}",
                role.key()
            );
        }

        // The reported values are gone from what the terminal was handed -
        // both halves of the issue's complaint, not just the yellow.
        assert!(
            !with_escapes.contains(OLD_YELLOW),
            "{scheme}: the pure-yellow warning reached the pane:\n{with_escapes:.2000}"
        );
        assert!(
            !with_escapes.contains(OLD_TEAL),
            "{scheme}: the old teal accent reached the pane:\n{with_escapes:.2000}"
        );
        // **The row must be able to tell the two palettes apart**, or it passes
        // on one frame no matter which theme is selected - which is exactly how
        // its first version passed while `/theme light` never took effect.
        let accent_sgr = sgr_for(&model, lca_ui::theme::Role::Accent);
        assert!(
            with_escapes.contains(&accent_sgr),
            "{scheme}: the pane is painting this palette's accent ({accent_sgr:?}) - \
             if it is not, the theme never switched:\n{with_escapes:.2000}"
        );
        let _ = pane;
    }

    session.send(&["C-u"]);
    session.send(&["/exit", "Enter"]);
    wait_for_pane_end(&session, std::time::Duration::from_secs(15));
}
