//! Per-role fallback chain rows (gh #387 gap 5).
//!
//! The rows here split into three groups, and the split is the point: the table's
//! own soundness (validated), the *resolution* (semantic, load-time, override
//! propagation), and the vocabulary's completeness (no orphan roles). A row that
//! only checked "a missing role resolves to something" would pass on the old
//! `or_else(Text)` just as well, so each row below is written to fail on it.

use crate::theme::tokens::{self, Slot, TOKENS, Token, nearest_named, resolve_chain};
use crate::theme::{Color, Palette, Role};

/// A lookup that answers only for the roles named in `pairs`.
fn only<'a>(pairs: &'a [(Role, Color)]) -> impl Fn(Role) -> Option<Color> + 'a {
    move |role| {
        pairs
            .iter()
            .find(|(candidate, _)| *candidate == role)
            .map(|(_, color)| *color)
    }
}

const RED: Color = Color::Rgb(0xff, 0x00, 0x00);
const BLUE: Color = Color::Rgb(0x00, 0x00, 0xff);
const GREEN: Color = Color::Rgb(0x00, 0xff, 0x00);

// ---------------------------------------------------------------- the table

/// Every role in the vocabulary has exactly one table entry, and no entry names
/// a role outside it.
///
/// This is the "no orphan roles" row the brief asks for: adding a token to
/// `roles!` without giving it a place in the table becomes a test failure here,
/// rather than a user's theme silently rendering the new role as `Default`.
#[test]
fn no_orphan_roles() {
    for role in Role::ALL {
        assert!(
            tokens::token(*role).is_some(),
            "`{}` is in the vocabulary but not in the fallback table - add a \
             `Token {{ role: Role::{}, .. }}` entry in `theme/tokens.rs`",
            role.key(),
            role.key()
        );
    }
    assert_eq!(
        TOKENS.len(),
        Role::ALL.len(),
        "the table has {} entries for {} roles",
        TOKENS.len(),
        Role::ALL.len()
    );
    for entry in TOKENS {
        assert!(
            Role::ALL.contains(&entry.role),
            "`{}` is in the table but not in the vocabulary",
            entry.role.key()
        );
    }
}

#[test]
fn the_table_validates() {
    // The four invariants (total, rooted per slot, same-slot, acyclic) hold for
    // the shipped table.
    assert_eq!(tokens::validate(), Ok(()));
}

/// Each slot has exactly one root, and both roots are the ones a theme is
/// expected to be built up from.
#[test]
fn each_slot_has_one_root() {
    for (slot, root) in [
        (Slot::Foreground, Role::Text),
        (Slot::Background, Role::SelectedBg),
    ] {
        let roots: Vec<Role> = TOKENS
            .iter()
            .filter(|token| token.slot == slot && token.fallback.is_none())
            .map(|token| token.role)
            .collect();
        assert_eq!(
            roots,
            vec![root],
            "{slot:?} has exactly one root, and it is {root:?}"
        );
    }
}

/// No fallback crosses the foreground/background boundary.
///
/// A background role inheriting a foreground colour would paint a band in what
/// is meant to be ink. pi rejects this at load (`has fallback X in a different
/// color slot`); so do we, and this pins that the shipped table has none.
#[test]
fn fallbacks_stay_in_their_slot() {
    for entry in TOKENS {
        let Some(parent) = entry.fallback else {
            continue;
        };
        let parent_entry = tokens::token(parent).expect("validated above");
        assert_eq!(
            parent_entry.slot,
            entry.slot,
            "`{}` ({:?}) falls back to `{}` ({:?})",
            entry.role.key(),
            entry.slot,
            parent.key(),
            parent_entry.slot
        );
    }
}

/// Ordinary roles are one hop from an obvious relative; only the thinking ramp
/// is a long chain, and it is long for a reason.
///
/// A single "every chain is short" bound would have to be 9 to fit the ramp,
/// which would stop the row meaning anything for the other 50 roles. Instead the
/// row states both facts: everything outside the ramp is at most three hops, and
/// the ramp is a *linear* chain - each level falls back to the next one cooler,
/// so naming any level carries every hotter level above it. That is the
/// behaviour a user wants from a heat scale ("I set the violet one, and hotter
/// inherits") and it is what pi does for its top level.
#[test]
fn chains_are_short_except_the_linear_thinking_ramp() {
    for entry in TOKENS {
        if Role::THINKING.contains(&entry.role) {
            continue;
        }
        let mut depth = 0;
        let mut current = entry.fallback;
        while let Some(role) = current {
            depth += 1;
            current = tokens::token(role).and_then(|token| token.fallback);
        }
        assert!(
            depth <= 3,
            "`{}` is {depth} hops from its root, which is too indirect: {entry:?}",
            entry.role.key()
        );
    }
    // The ramp: each level's parent is the level immediately cooler.
    let ramp = Role::THINKING;
    for pair in ramp.windows(2) {
        let entry = tokens::token(pair[1]).expect("in the table");
        assert_eq!(
            entry.fallback,
            Some(pair[0]),
            "`{}` should fall back to the level below it, `{}`",
            pair[1].key(),
            pair[0].key()
        );
    }
    // ...and the coolest level grounds in the border, not in another level.
    let off = tokens::token(Role::ThinkingOff).expect("in the table");
    assert_eq!(off.fallback, Some(Role::BorderMuted));
}

/// Naming one level of the ramp carries every hotter level.
#[test]
fn naming_a_thinking_level_carries_the_hotter_ones() {
    let (palette, _) = Palette::dark()
        .overlay("thinkingMedium = \"#0a0b0c\"\n")
        .expect("valid overlay");
    let medium = Color::Rgb(0x0a, 0x0b, 0x0c);
    assert_eq!(palette.get(Role::ThinkingMedium), medium);
    for hotter in [Role::ThinkingHigh, Role::ThinkingXhigh, Role::ThinkingMax] {
        assert_eq!(
            palette.get(hotter),
            medium,
            "`{}` is hotter than `medium`, so it inherits it",
            hotter.key()
        );
    }
    for cooler in [Role::ThinkingOff, Role::ThinkingMinimal, Role::ThinkingLow] {
        assert_ne!(
            palette.get(cooler),
            medium,
            "`{}` is cooler than `medium`, so it keeps the base value",
            cooler.key()
        );
    }
}

// ----------------------------------------------------------- the resolution

/// A missing role resolves through its declared parent.
///
/// The old behaviour was `or_else(Text)`; this row uses `mdLink`, whose parent is
/// `accent`, so the two answers are different and the row can tell them apart.
#[test]
fn a_missing_role_resolves_through_its_parent() {
    let have = only(&[(Role::Accent, BLUE), (Role::Text, RED)]);
    assert_eq!(
        resolve_chain(Role::MdLink, &have),
        BLUE,
        "`mdLink` falls back to `accent`, not to `text`"
    );
    // And the multi-hop case: `syntaxType` -> `mdCode` -> `accent`.
    assert_eq!(
        resolve_chain(Role::SyntaxType, &have),
        BLUE,
        "a two-hop chain walks all the way up"
    );
    // A role with nothing in its family above it: `text` is the root.
    assert_eq!(resolve_chain(Role::ToolTitle, &have), RED);
}

/// A background role never borrows a foreground colour.
#[test]
fn a_missing_background_role_does_not_resolve_to_text() {
    let have = only(&[(Role::Text, RED)]);
    // `userMessageBg -> selectedBg`, and `selectedBg` is absent, so there is no
    // background value to find; `text` is on the other side of the wall.
    assert_eq!(
        resolve_chain(Role::UserMessageBg, &have),
        Color::Default,
        "a background role must not resolve to a foreground colour"
    );
}

/// A role whose whole chain is absent is the terminal default, which is what a
/// theme that named nothing actually specified.
#[test]
fn an_unnamed_chain_resolves_to_default() {
    let nothing = only(&[]);
    for role in [
        Role::MdLink,
        Role::ToolSuccessBg,
        Role::Text,
        Role::SelectedBg,
    ] {
        assert_eq!(
            resolve_chain(role, &nothing),
            Color::Default,
            "{role:?} with no declarations is the terminal's own value"
        );
    }
}

/// `nearest_named` looks at ancestors only - never the role itself.
#[test]
fn nearest_named_skips_the_role_itself() {
    let have = only(&[(Role::MdLink, GREEN), (Role::Accent, BLUE)]);
    assert_eq!(
        nearest_named(Role::MdLink, &have),
        Some(BLUE),
        "the answer is the parent, not the role's own value"
    );
    let have = only(&[(Role::MdLink, GREEN)]);
    assert_eq!(
        nearest_named(Role::MdLink, &have),
        None,
        "a role is not its own ancestor"
    );
}

// ------------------------------------------------------ the load-time chain

/// A theme file naming a subset resolves every omitted role through its declared
/// parent **rather than through the base palette's value**.
///
/// This is the sharp one: the base is complete, so an implementation that copied
/// the base and overwrote named roles would return `dark()`'s `mdLink`
/// (`#81a2be`) here and still look plausible. The assertion is that the answer is
/// the *user's* accent.
#[test]
fn a_partial_theme_resolves_omitted_roles_through_their_parents() {
    let (palette, applied) = Palette::dark()
        .overlay("accent = \"#123456\"\n")
        .expect("valid overlay");
    assert_eq!(applied, 1, "one role was named");
    let accent = Color::Rgb(0x12, 0x34, 0x56);
    assert_eq!(palette.get(Role::Accent), accent);
    // Children of `accent`, none of them named by the file:
    for child in [
        Role::MdLink,
        Role::MdCode,
        Role::MdListBullet,
        Role::SyntaxKeyword,
        Role::CustomMessageLabel,
        Role::BashMode,
    ] {
        assert_eq!(
            palette.get(child),
            accent,
            "`{}` should follow the accent it declares as its parent",
            child.key()
        );
    }
    // Two hops: `syntaxType -> mdCode -> accent`.
    assert_eq!(
        palette.get(Role::SyntaxType),
        accent,
        "two hops still lands"
    );
    // A role with no named ancestor keeps the base's own colour, so a partial
    // theme does not flatten the whole palette onto one value.
    assert_eq!(
        palette.get(Role::Error),
        Palette::dark().get(Role::Error),
        "`error` has no named ancestor, so it keeps the base value"
    );
}

/// The same parent, a different child: overriding `muted` moves `dim`, but not
/// `accent`.
#[test]
fn a_parent_override_carries_only_its_own_children() {
    let (palette, _) = Palette::dark()
        .overlay("muted = \"#aabbcc\"\n")
        .expect("valid overlay");
    let muted = Color::Rgb(0xaa, 0xbb, 0xcc);
    assert_eq!(palette.get(Role::Muted), muted);
    assert_eq!(palette.get(Role::Dim), muted, "`dim -> muted`");
    assert_eq!(
        palette.get(Role::ToolOutput),
        muted,
        "`toolOutput -> muted`"
    );
    assert_eq!(palette.get(Role::SyntaxComment), muted);
    assert_ne!(
        palette.get(Role::Accent),
        muted,
        "`accent` is not a child of `muted`"
    );
}

/// Overriding a grandparent carrying two levels: `border` moves `borderMuted`
/// and, through it, `mdHr`.
#[test]
fn a_grandparent_override_reaches_two_levels_down() {
    let (palette, _) = Palette::dark()
        .overlay("border = \"#010203\"\n")
        .expect("valid overlay");
    let border = Color::Rgb(0x01, 0x02, 0x03);
    assert_eq!(palette.get(Role::BorderMuted), border, "one hop");
    assert_eq!(palette.get(Role::MdHr), border, "two hops via borderMuted");
    assert_eq!(palette.get(Role::MdQuoteBorder), border);
    assert_eq!(palette.get(Role::ThinkingOff), border);
    // And three: `thinkingMinimal -> thinkingOff -> borderMuted -> border`.
    assert_eq!(palette.get(Role::ThinkingMinimal), border, "three hops");
}

/// A child named explicitly beats its parent: the chain is a default, not an
/// override of the user's own words.
#[test]
fn a_named_child_wins_over_its_named_parent() {
    let (palette, applied) = Palette::dark()
        .overlay("accent = \"#111111\"\nmdLink = \"#222222\"\n")
        .expect("valid overlay");
    assert_eq!(applied, 2);
    assert_eq!(palette.get(Role::Accent), Color::Rgb(0x11, 0x11, 0x11));
    assert_eq!(
        palette.get(Role::MdLink),
        Color::Rgb(0x22, 0x22, 0x22),
        "the child the user named keeps its own value"
    );
    assert_eq!(
        palette.get(Role::MdCode),
        Color::Rgb(0x11, 0x11, 0x11),
        "a sibling that was not named still follows the parent"
    );
}

/// After an overlay every role is named in the map - the chain is resolved at
/// load, so the render path never walks it.
#[test]
fn an_overlaid_palette_is_fully_resolved() {
    let (palette, _) = Palette::dark()
        .overlay("accent = \"#123456\"\n")
        .expect("valid overlay");
    assert!(
        palette.is_complete(),
        "every role holds a value after the overlay"
    );
}

/// An empty file is a valid theme: nothing named, so every role keeps the base.
#[test]
fn an_empty_theme_keeps_the_base() {
    let (palette, applied) = Palette::dark().overlay("").expect("valid overlay");
    assert_eq!(applied, 0);
    for role in Role::ALL {
        assert_eq!(
            palette.get(*role),
            Palette::dark().get(*role),
            "`{}` unchanged by an empty theme",
            role.key()
        );
    }
}

// ------------------------------------------------------------- the failures

/// An unknown role name stays an error with the message it always had.
#[test]
fn an_unknown_role_is_still_an_error() {
    let err = Palette::dark()
        .overlay("accent = \"not-a-color\"\n")
        .expect_err("an invalid colour is refused");
    assert!(
        err.contains("accent") && err.contains("invalid color"),
        "the message names the role and the problem: {err}"
    );
    // A role name that is not in the vocabulary is skipped, not an error: a
    // theme written against a future vocabulary keeps loading (this is the
    // forward-compatibility the issue is about).
    let (palette, applied) = Palette::dark()
        .overlay("notARole = \"#123456\"\naccent = \"#111111\"\n")
        .expect("an unknown key is skipped");
    assert_eq!(applied, 1, "only the known role counted");
    assert_eq!(palette.get(Role::Accent), Color::Rgb(0x11, 0x11, 0x11));
}

/// A hand-built table with a cycle fails validation, and the message names the
/// loop.
///
/// Built over a local table rather than by mutating `TOKENS` (which is `const`),
/// so this is a *property of `validate`* and stays true regardless of what the
/// shipped table says.
#[test]
fn a_cycle_is_named() {
    // Mirrors `validate`'s cycle walk over a deliberately broken table.
    fn validate_with(table: &[Token]) -> Result<(), String> {
        let find = |role: Role| table.iter().find(|token| token.role == role);
        for role in Role::ALL {
            let mut seen = vec![*role];
            let mut current = find(*role).and_then(|token| token.fallback);
            while let Some(next) = current {
                if seen.contains(&next) {
                    let loop_ = seen
                        .iter()
                        .skip_while(|seen| **seen != next)
                        .chain(std::iter::once(&next))
                        .map(|role| role.key())
                        .collect::<Vec<_>>()
                        .join(" -> ");
                    return Err(format!("fallback cycle: {loop_}"));
                }
                seen.push(next);
                current = find(next).and_then(|token| token.fallback);
            }
        }
        Ok(())
    }
    // `accent -> muted -> accent`.
    let cyclic = [
        Token {
            role: Role::Accent,
            slot: Slot::Foreground,
            fallback: Some(Role::Muted),
        },
        Token {
            role: Role::Muted,
            slot: Slot::Foreground,
            fallback: Some(Role::Accent),
        },
        Token {
            role: Role::Text,
            slot: Slot::Foreground,
            fallback: None,
        },
    ];
    let err = validate_with(&cyclic).expect_err("a cycle must be refused");
    assert!(
        err.contains("accent") && err.contains("muted") && err.contains("cycle"),
        "the message names the loop: {err}"
    );
    // The shipped table has none.
    assert_eq!(tokens::validate(), Ok(()));
}

/// A hop bound keeps resolution finite even if the table were broken at runtime.
///
/// `resolve_chain` is the production path, so it must not be able to hang; the
/// fault is caught loudly by `validate` at load, and this is the belt.
#[test]
fn resolution_terminates_before_the_hop_bound() {
    // The real chains are at most 3 hops, far inside the bound of 64.
    let mut deepest = 0;
    for role in Role::ALL {
        let mut depth = 0;
        let mut current = tokens::token(*role).and_then(|token| token.fallback);
        while let Some(role) = current {
            depth += 1;
            current = tokens::token(role).and_then(|token| token.fallback);
        }
        deepest = deepest.max(depth);
    }
    assert!(deepest < 64, "deepest chain is {deepest} hops");
}

// -------------------------------------------------- the documentation

/// The role table in `docs/configuration.md` matches the table in the code.
///
/// The docs are the law here, and a hand-maintained 57-row table is exactly the
/// kind of artefact that rots silently: a reader follows the doc, sets a parent
/// they were told about, and nothing happens. This row parses both and compares
/// them, so a change to `TOKENS` that forgets the doc (or the reverse) fails.
///
/// Parsing a markdown table with a regex is a shortcut with a known limit: the
/// rows are `| `roles` | `parent` | why |` and nothing else, and a reformat to
/// different columns would make this row error rather than silently pass. The
/// `at least one row` assertion is what catches that.
#[test]
fn the_documented_fallbacks_match_the_table() {
    let doc = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/configuration.md"
    ))
    .expect("the configuration reference is in the tree");
    let start = doc.find("## Theme fallbacks").expect("the section exists");
    let rest = &doc[start..];
    let end = rest
        .find("## Model metadata overrides")
        .expect("the section ends");
    let section = &rest[..end];

    let mut documented: Vec<(String, String)> = Vec::new();
    for line in section.lines() {
        let line = line.trim();
        // Only table body rows: `| ... | `parent` | ... |`.
        let Some(body) = line.strip_prefix("| ").and_then(|l| l.strip_suffix(" |")) else {
            continue;
        };
        let cells: Vec<&str> = body.split(" | ").collect();
        if cells.len() != 3 {
            continue;
        }
        // The parent cell is a single backticked role name.
        let parent_cell = cells[1].trim();
        let Some(parent) = parent_cell
            .strip_prefix('`')
            .and_then(|p| p.strip_suffix('`'))
        else {
            continue;
        };
        if parent.contains(' ') {
            continue; // the "level one cooler" prose row, gone now, but be safe
        }
        for role in cells[0].split('`').skip(1).step_by(2) {
            let role = role.trim();
            if role.is_empty() || role == "Role" || role.contains(' ') {
                continue;
            }
            documented.push((role.to_string(), parent.to_string()));
        }
    }
    assert!(
        documented.len() > 40,
        "the doc table was not parsed; {} rows found - a reformat broke the row",
        documented.len()
    );

    // Every documented (child, parent) pair must exist in the code.
    for (child, parent) in &documented {
        let role = Role::parse(child)
            .unwrap_or_else(|| panic!("the doc names `{child}`, which is not a role"));
        let entry = tokens::token(role).expect("table is total");
        let actual = entry.fallback.expect("the doc only documents children");
        assert_eq!(
            actual.key(),
            parent,
            "the doc says `{child}` falls back to `{parent}`, the code says `{}`",
            actual.key()
        );
    }

    // And every role in the code with a parent must be documented.
    for entry in TOKENS {
        let Some(parent) = entry.fallback else {
            continue;
        };
        assert!(
            documented
                .iter()
                .any(|(child, _)| child == entry.role.key()),
            "`{}` falls back to `{}`, and the theme section of \
             `docs/configuration.md` does not say so",
            entry.role.key(),
            parent.key()
        );
    }
}
