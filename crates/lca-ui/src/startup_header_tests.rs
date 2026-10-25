//! gh #399 rows: the three-line header, its live key hints, and the width ladder.

use super::*;
use crate::theme::Theme;

fn version() -> VersionInfo {
    VersionInfo::new("0.6.1", "0.6")
}

/// Strip every SGR sequence, so a row can assert text without escapes.
fn plain(row: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for ch in row.chars() {
        if ch == '\u{1b}' {
            skipping = true;
        } else if skipping {
            if ch == 'm' {
                skipping = false;
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn composed(width: usize, apple: bool) -> Vec<String> {
    compose(
        &Theme::plain(),
        &KeybindingsManager::new(),
        &version(),
        width,
        apple,
    )
}

#[test]
fn the_header_is_the_owners_three_line_composition() {
    // Every element the owner drew, in the order drawn: mark + version, then the
    // hint line, then the expand nudge with the onboarding line.
    let rows = composed(200, false);
    let text = rows.iter().map(|row| plain(row)).collect::<Vec<_>>();

    assert_eq!(text.len(), 3, "three rows, as sketched: {text:?}");
    assert!(
        text[0].contains("v0.6.1 (abi 0.6)"),
        "row 1 carries the version: {:?}",
        text[0]
    );
    assert!(
        text[0].starts_with('█'),
        "the mark leads row 1: {:?}",
        text[0]
    );
    assert!(
        text[1].contains("escape interrupt"),
        "row 2 carries the hints: {:?}",
        text[1]
    );
    assert!(
        text[2].contains("Press ctrl+o to show full startup help."),
        "row 3 carries the expand nudge: {:?}",
        text[2]
    );
    assert!(
        text[2].contains("LCA can explain its own features"),
        "row 3 also carries the onboarding nudge: {:?}",
        text[2]
    );
}

#[test]
fn every_key_on_the_line_comes_from_the_live_registry() {
    // #410's lesson, as a row: rebind the actions and the line follows. A hint
    // that spelled `ctrl+c` itself would fail here, which is the point.
    let mut user = std::collections::BTreeMap::new();
    user.insert("app.interrupt".to_string(), vec!["ctrl+q".to_string()]);
    user.insert("app.clear".to_string(), vec!["ctrl+x".to_string()]);
    user.insert("app.tools.expand".to_string(), vec!["ctrl+e".to_string()]);
    let rebound = KeybindingsManager::with_user_bindings(user.clone());

    let line = hint_line(&rebound);
    assert!(
        line.contains("ctrl+q interrupt"),
        "the new interrupt key: {line}"
    );
    assert!(
        !line.contains("escape interrupt"),
        "the old key is gone: {line}"
    );
    assert!(
        line.contains("ctrl+x ctrl+x clear/exit"),
        "clear/exit follows too: {line}"
    );
    assert!(line.contains("ctrl+e more"), "and the expand key: {line}");
    assert_eq!(
        press_hint(&rebound),
        "Press ctrl+e to show full startup help.",
        "the nudge names the same live key"
    );
}

#[test]
fn an_unbound_action_is_omitted_rather_than_rendered_blank() {
    // A blank key press reads as a bug; dropping the hint is the honest answer,
    // and the row forbids the ` interrupt` shape.
    let mut user = std::collections::BTreeMap::new();
    user.insert("app.interrupt".to_string(), Vec::new());
    user.insert("app.tools.expand".to_string(), Vec::new());
    let unbound = KeybindingsManager::with_user_bindings(user.clone());

    let line = hint_line(&unbound);
    assert!(
        !line.contains("interrupt"),
        "an unbound interrupt drops its hint: {line}"
    );
    assert!(
        !line.contains("more"),
        "an unbound expand drops its hint: {line}"
    );
    assert!(
        !line.contains("  "),
        "no blank key press leaves a double space: {line:?}"
    );
    assert_eq!(
        press_hint(&unbound),
        "",
        "the nudge would name a dead key, so it is empty"
    );

    // And the composition omits that whole line rather than printing an empty row.
    let rows = compose(&Theme::plain(), &unbound, &version(), 200, false);
    let text = rows.iter().map(|row| plain(row)).collect::<Vec<_>>();
    assert!(
        !text.iter().any(|row| row.trim_end().ends_with('·')),
        "no dangling separator: {text:?}"
    );
}

#[test]
fn a_narrow_terminal_stacks_the_wordmark_instead_of_wrapping_the_hints() {
    // Below the full composition the header does not wrap: it drops to the
    // wordmark and puts the text rows beneath. A wrapped hint line reads as a
    // rendering fault.
    let narrow = composed(40, false);
    let text = narrow.iter().map(|row| plain(row)).collect::<Vec<_>>();
    assert_eq!(text[0], "LCA", "the wordmark leads: {text:?}");
    assert!(
        text.iter().any(|row| row.contains("escape interrupt")),
        "the hints survive on their own row: {text:?}"
    );
    // The contract this row pins: **the mark** never wraps. Text rows go back to
    // the transcript as one logical line and the `Raw` entry wraps them, which is
    // what a 79-column hint line in a 40-column terminal should do. A mark split
    // across rows is corruption; a sentence wrapped is ordinary.
    for row in text.iter().filter(|row| row.contains(['█', '▀', '▄'])) {
        assert!(
            visible_width(row) <= 40,
            "a mark row must fit the terminal: {row:?} is {}",
            visible_width(row)
        );
    }
    // And the mark gets its own row rather than sharing one, so nothing is left in
    // a column that no longer lines up.
    assert_eq!(text[0], "LCA", "the wordmark leads alone: {text:?}");
    for row in text.iter().skip(1) {
        assert!(
            !row.contains(['█', '▀', '▄']),
            "no block glyph shares a text row: {row:?}"
        );
    }
    // The hint line is handed back whole and wraps at render (see above), so the
    // thing to forbid is *breaking it up ourselves* into invented fragments.
    assert_eq!(
        text.iter().filter(|row| row.contains("·")).count(),
        1,
        "the hint line stays one logical row: {text:?}"
    );
}

#[test]
fn a_very_narrow_terminal_prints_the_version_alone() {
    // The floor: the version still has to be readable, so the mark goes and the
    // text stays rather than truncating the one line that identifies the build.
    let tiny = composed(12, false);
    let text = tiny.iter().map(|row| plain(row)).collect::<Vec<_>>();
    assert_eq!(
        text,
        vec!["LCA", "v0.6.1 (abi 0.6)"],
        "version only: {text:?}"
    );
    // The version is printed **whole**. At 12 columns it is wider than the
    // terminal and the terminal itself will clip it; what the header must not do is
    // truncate it, because a shortened version is a false statement about which
    // build is running. The mark, being art, does have to fit.
    assert!(
        text[1].ends_with("(abi 0.6)"),
        "the version is never truncated by us: {text:?}"
    );
    assert!(visible_width(&text[0]) <= 12, "the mark fits: {text:?}");
}

#[test]
fn apple_terminal_never_shows_half_blocks_in_the_header() {
    // The fallback reaches the composition, not just the logo module: even at a
    // wide terminal, Apple's Terminal gets the wordmark and no `▀`/`▄`/`█`.
    let rows = composed(200, true);
    let text = rows.iter().map(|row| plain(row)).collect::<Vec<_>>();
    assert_eq!(text[0].trim_start(), "LCA v0.6.1 (abi 0.6)", "{text:?}");
    for glyph in ['▀', '▄', '█'] {
        assert!(
            !text.iter().any(|row| row.contains(glyph)),
            "no half block reaches Apple_Terminal: {text:?}"
        );
    }
}

#[test]
fn the_mark_keeps_its_own_colours_in_the_composition() {
    // The separation again, at the header's level: the mark's cells keep the three
    // logo hex values while the text is dim, so a theme switch cannot reach them.
    let theme = Theme::colored();
    let rows = compose(&theme, &KeybindingsManager::new(), &version(), 200, false);
    let joined = rows.join("");
    for hex in ["38;2;98;174;220", "38;2;226;140;0", "38;2;236;205;0"] {
        assert!(joined.contains(hex), "the mark keeps {hex}: {joined:?}");
    }
    // And the text beside it is a dim role, so the two are distinguishable.
    // `theme.dim` is the accessor with the attribute on it (`2;38;2;…`), not the
    // bare `role(Dim)` colour - a row caught that difference. Derived from the
    // theme rather than pinned, because `dim` is now a calibrated neutral
    // (gh #387 gap 3) and a literal would need editing whenever it moves.
    let dim = (theme.dim)("x");
    let dim_sgr = dim.split('x').next().unwrap_or_default();
    assert!(
        !dim_sgr.is_empty() && joined.contains(dim_sgr),
        "the text rows are dim ({dim_sgr:?}), as pi's header is: {joined:?}"
    );
}

#[test]
fn a_wide_terminal_puts_the_mark_beside_the_text_not_above_it() {
    // The owner's composition is *beside*: three mark rows left of three text
    // rows. How many rows the block has is the proof - stacking would print at
    // least four.
    let rows = composed(200, false);
    assert_eq!(rows.len(), 3, "beside, not stacked: {rows:?}");
    let text = rows.iter().map(|row| plain(row)).collect::<Vec<_>>();
    for (index, row) in text.iter().enumerate() {
        assert!(
            row.starts_with(['█', '▀', '▄'])
                || row.starts_with("█ ")
                || row.chars().next().is_some_and(|c| "█▀▄ ".contains(c)),
            "row {index} begins in the mark's column: {row:?}"
        );
    }
}
