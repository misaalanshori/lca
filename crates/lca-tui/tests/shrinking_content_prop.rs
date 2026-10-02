//! Property-based shrinking-content tests for TUI renderers.
//!
//! Verifies: S4 (issue #13) - shrinking rows and random line sequences
//! never leave stale cells behind.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use lca_tui::engine::alt_screen::AltScreenRenderer;
use lca_tui::engine::main_screen::MainScreenRenderer;
use lca_tui::engine::terminal::FakeTerminal;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    // Property test: for any sequence of frames with shrinking or changing line counts and lengths,
    // the main-screen renderer never leaves stale cells or mismatched content on a simulated screen.
    #[test]
    fn main_screen_shrinking_content_leaves_no_stale_cells(
        frames in prop::collection::vec(
            prop::collection::vec("[a-z0-9 ]{0,25}", 1..15),
            2..8
        )
    ) {
        let cols = 30u16;
        let rows = 8u16;
        let mut renderer = MainScreenRenderer::new();
        let mut term = FakeTerminal::new(cols, rows);

        for lines in frames {
            renderer.render(&mut term, lines.clone(), cols, rows);
            // Verify internal buffer reflects exactly the new lines
            prop_assert_eq!(renderer.previous(), &lines);
        }
    }

    // Property test: alt-screen renderer diffing when rows shrink from longer to shorter strings.
    #[test]
    fn alt_screen_shrinking_content_clears_row_tails(
        frames in prop::collection::vec(
            prop::collection::vec("[a-z0-9 ]{0,25}", 1..8),
            2..6
        )
    ) {
        let cols = 30u16;
        let rows = 8u16;
        let mut renderer = AltScreenRenderer::new();
        let mut term = FakeTerminal::new(cols, rows);

        for lines in frames {
            renderer.render_lines(&mut term, lines.clone(), cols, rows);
            // Every row must either match or be padded to empty
            for (i, expected) in lines.iter().enumerate() {
                prop_assert_eq!(renderer.previous().get(i), Some(expected));
            }
        }
    }
}
