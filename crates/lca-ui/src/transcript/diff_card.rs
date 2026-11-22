//! The diff card's body: chrome-free rows, a line-number gutter, and the 1↔1 word
//! spotlight (gh #392 slices A and B).
//!
//! # What was wrong
//!
//! The card painted the tool's unified diff line for line: `--- a/x`, `+++ b/x` and
//! `@@ -40,7 +40,7 @@` came out as **context rows**, so a reader scrolling a change saw a
//! wall of punctuation that said nothing - the file is already named in the card header -
//! and nothing on any row said **where in the file** the change landed. A one-word fix
//! was two solid-coloured lines (`- old` / `+ new`) that the eye had to re-read whole; at
//! 60 columns a wide row silently wrapped under whatever gutter there was.
//!
//! pi's renderer (`diff.ts`, `d1-04`, `c4-55`) shows three colours and nothing else:
//! removed, added, context - each row prefixed by the file line it occupies, in a dim
//! gutter of one fixed width, with the body clipped to the space that is left. And a
//! **single** removed line paired with a **single** added line gets its changed words
//! painted in inverse, because that is the one shape where the word is knowable cheaply.
//!
//! # What this file owns
//!
//! * [`parse`] - the unified diff into [`Row`]s that carry side line numbers, dropping
//!   chrome. A malformed or truncated patch is still parsed as far as it goes: a
//!   streaming hunk must render progressively, never blank (`#392` gap 7).
//! * [`render`] - those rows to styled lines, with the gutter, the clipping, and the
//!   inverse word spans for a 1↔1 replacement.
//!
//! # The two rules that are easy to get wrong
//!
//! * **Per-channel closes only.** The gutter is `Role::Muted` and the body is one of the
//!   three diff roles; each `Theme::role` closes the SGR channel it opened (`39`), never
//!   all of them (`0m`), so the two styles compose. The inverse span closes with `27`.
//! * **Indentation stays outside the inverse span.** A word diff that inverted the
//!   leading whitespace paints solid bars down the left of a one-line change, which is
//!   what pi's `diffWords` usage avoids by emitting the indent before the first span.

use lca_tui::engine::text::{truncate_to_width, visible_width};

use crate::theme::{Role, Theme};

/// One diff row after chrome removal: what kind of change it is, the file line it sits
/// on for each side, and its text without the unified-diff marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Row {
    /// The row's kind, which decides its role.
    pub(super) kind: Kind,
    /// The line number on the **old** side (`None` for an added row).
    pub(super) old: Option<usize>,
    /// The line number on the **new** side (`None` for a removed row).
    pub(super) new: Option<usize>,
    /// The row's text, with the leading `-`/`+`/` ` marker removed.
    pub(super) text: String,
}

/// A diff row's kind: pi's three colours and no fourth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// A line the change added.
    Added,
    /// A line the change removed.
    Removed,
    /// An unchanged line kept for context.
    Context,
}

impl Kind {
    /// The role this row paints in.
    fn role(self) -> Role {
        match self {
            Kind::Added => Role::ToolDiffAdded,
            Kind::Removed => Role::ToolDiffRemoved,
            Kind::Context => Role::ToolDiffContext,
        }
    }

    /// The marker a reader sees in the gutter: `-`, `+`, or a space.
    fn marker(self) -> char {
        match self {
            Kind::Added => '+',
            Kind::Removed => '-',
            Kind::Context => ' ',
        }
    }
}

/// Is this line the unified diff's file/hunk **chrome** rather than content?
///
/// Three shapes plus one git quirk: `--- a/x`, `+++ b/x`, `@@ -1,3 +1,3 @@`, and the
/// `\ No newline at end of file` marker a diff may carry. None of them is a line of the
/// file, so none of them may be printed as one - and each is a row the old renderer
/// painted as *context*, i.e. as if the file contained it.
fn is_chrome(line: &str) -> bool {
    line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("@@")
        || line.starts_with("\\ No newline")
}

/// The two start numbers a hunk header declares.
///
/// `@@ -40,7 +42,9 @@ optional section` → `(40, 42)`. A header without counts (`@@ -40 +42 @@`) is the
/// one-line form unified diff allows, and a header this function cannot read answers
/// `None` so the caller keeps its previous counters rather than inventing numbers.
fn hunk_starts(line: &str) -> Option<(usize, usize)> {
    let body = line.strip_prefix("@@")?;
    let end = body.find("@@")?;
    let mut starts = body[..end].split_whitespace().filter_map(|field| {
        let rest = field
            .strip_prefix('-')
            .or_else(|| field.strip_prefix('+'))?;
        let digits = rest.split(',').next()?;
        digits.parse::<usize>().ok()
    });
    match (starts.next(), starts.next()) {
        (Some(old), Some(new)) => Some((old, new)),
        _ => None,
    }
}

/// Parse a unified diff into numbered, chrome-free rows.
///
/// Deterministic and tolerant by design: a patch whose hunk header is missing or whose
/// last hunk is still being streamed still yields every row it *can* describe, with
/// `None` numbers where the header never told us - a row with no number is honest, a
/// row with a wrong number is not.
pub(super) fn parse(diff: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    // The counters a hunk header sets; `None` until one is seen.
    let mut old_next: Option<usize> = None;
    let mut new_next: Option<usize> = None;
    for line in diff.lines() {
        if is_chrome(line) {
            if let Some((old, new)) = hunk_starts(line) {
                old_next = Some(old);
                new_next = Some(new);
            }
            continue;
        }
        let (kind, text) = match line.chars().next() {
            Some('-') => (Kind::Removed, &line[1..]),
            Some('+') => (Kind::Added, &line[1..]),
            // A context row normally begins with a space; a bare empty line is a context
            // row whose space the producer trimmed, and is treated the same way.
            Some(' ') => (Kind::Context, &line[1..]),
            None => (Kind::Context, ""),
            // Not a marker at all: prose the producer put in the same buffer (an `edit`
            // failure's message reaches `Body::classify` as no diff, so this is the
            // malformed-stream case). Render it as context rather than dropping it - a
            // dropped line in a change card is worse than an unlabelled one.
            Some(_) => (Kind::Context, line),
        };
        let (old, new) = match kind {
            Kind::Removed => (old_next, None),
            Kind::Added => (None, new_next),
            Kind::Context => (old_next, new_next),
        };
        if kind != Kind::Added
            && let Some(n) = old_next.as_mut()
        {
            *n += 1;
        }
        if kind != Kind::Removed
            && let Some(n) = new_next.as_mut()
        {
            *n += 1;
        }
        rows.push(Row {
            kind,
            old,
            new,
            text: text.to_string(),
        });
    }
    rows
}

/// Split a line into `(separator, token)` pairs: the whitespace before a token, then the
/// token itself.
///
/// A token is a run of word characters **or a single other character**, which is what
/// jsdiff's `diffWords` does and what makes `return 1;` → `return 2;` mark the `1` alone
/// rather than `1;` (measured: without the punctuation split the span covered the
/// semicolon, saying "the semicolon changed too").
///
/// Returning the separators rather than losing them is what lets the word spotlight paint
/// only the tokens: the indentation is emitted *before* any inverse span, so a one-word
/// change does not paint bars where nothing changed.
fn words(line: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = line;
    loop {
        let sep_end = rest
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len());
        let (sep, tail) = rest.split_at(sep_end);
        let Some(first) = tail.chars().next() else {
            if !sep.is_empty() {
                out.push((sep, ""));
            }
            return out;
        };
        let word_end = if first.is_alphanumeric() || first == '_' {
            tail.find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(tail.len())
        } else {
            first.len_utf8()
        };
        let (word, tail) = tail.split_at(word_end);
        out.push((sep, word));
        rest = tail;
    }
}

/// Paint the changed words of a 1↔1 replacement in inverse, leaving the rest alone.
///
/// The pair is matched by its **word spans'** common head and tail: everything between
/// them differs, and everything outside them is identical by construction - the cheap,
/// deterministic answer for exactly the one shape the issue allows (a single removed line
/// next to a single added line). A general LCS would be a different, larger thing, and
/// pi restricts to 1↔1 for the same reason.
///
/// A side whose differing middle is empty (a word inserted or deleted) gets no inverse
/// span: painting a zero-width run is invisible at best.
fn spotlight(old: &str, new: &str) -> (String, String) {
    const INVERSE_ON: &str = "\x1b[7m";
    const INVERSE_OFF: &str = "\x1b[27m";
    let old_parts = words(old);
    let new_parts = words(new);
    let head = old_parts
        .iter()
        .zip(new_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let tail = old_parts
        .iter()
        .rev()
        .zip(new_parts.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        // The head and the tail must not overlap on the shorter side.
        .min(old_parts.len().saturating_sub(head))
        .min(new_parts.len().saturating_sub(head));
    let paint = |parts: &[(&str, &str)]| -> String {
        let mut out = String::new();
        for (index, (sep, word)) in parts.iter().enumerate() {
            // The separator never rides inside the span: that is what keeps the indent
            // bar off the change.
            out.push_str(sep);
            if word.is_empty() {
                continue;
            }
            let changed = index >= head && index < parts.len().saturating_sub(tail);
            if changed {
                out.push_str(INVERSE_ON);
                out.push_str(word);
                out.push_str(INVERSE_OFF);
            } else {
                out.push_str(word);
            }
        }
        out
    };
    (paint(&old_parts), paint(&new_parts))
}

/// Clip plain text to `width`, ending in an ellipsis when it had to cut.
///
/// Deliberately **not** [`truncate_to_width`] for this case: that helper finalises a
/// truncation with `\x1b[0m`, which clears *every* channel - including the tool card's
/// state background painted outside this row. The per-channel rule this card follows
/// (`\x1b[39m` for the foreground, `\x1b[27m` for inverse) is the opposite, and a plain
/// (escape-free) row needs no finalisation at all.
///
/// The loop is quadratic in the line's width, which is fine for the shape it serves: a
/// diff row is tens of characters, and correctness here beats a micro-optimisation that
/// would have to re-implement grapheme clustering.
fn clip_plain(text: &str, width: usize) -> String {
    if visible_width(text) <= width {
        return text.to_string();
    }
    let target = width.saturating_sub(1);
    let mut out = String::new();
    for ch in text.chars() {
        let mut next = out.clone();
        next.push(ch);
        if visible_width(&next) > target {
            break;
        }
        out = next;
    }
    out.push('…');
    out
}

/// Make a styled row **per-channel only**: drop any all-channel reset and close an inverse
/// span left open by a clip.
///
/// `truncate_to_width` is the right tool for clipping styled text (it tracks escape state
/// across the cut) but it finalises with `\x1b[0m`; a spotlit row that is also clipped
/// would therefore punch a hole in the card's state background. Stripping the resets is
/// safe here because every channel this body opens is closed per-channel: the caller's
/// `Theme::role` closes the foreground with `39`, and the balance below closes `7`.
fn per_channel_only(styled: &str) -> String {
    let mut out = styled.replace("\x1b[0m", "");
    let opens = out.matches("\x1b[7m").count();
    let closes = out.matches("\x1b[27m").count();
    for _ in closes..opens {
        out.push_str("\x1b[27m");
    }
    out
}

/// Render the body's rows: gutter, role, clipping, and the 1↔1 spotlight.
///
/// Returns one styled line per row (excluding the `… N more lines` marker, which the
/// caller owns because it needs the keybinding hint). `inner` is the card's usable width;
/// the gutter takes a fixed slice of it and the body is clipped to the remainder.
pub(super) fn render(rows: &[Row], inner: usize, theme: &Theme) -> Vec<String> {
    // The widest number any row carries decides the gutter, so every row lines up and no
    // row's body shifts by a digit.
    let widest = rows
        .iter()
        .filter_map(|row| row.new.or(row.old))
        .max()
        .unwrap_or(0);
    let digits = widest.to_string().len().max(1);
    // `- ` + number + one space; a context row's marker is a space, so the width holds.
    let gutter = digits + 3;
    // `inner` is the card's usable width and every row is prefixed with two spaces by the
    // caller, so the gutter and the body share what is left of *that*.
    let body_width = inner.saturating_sub(2).saturating_sub(gutter).max(1);

    // Which rows are one half of a 1↔1 replacement: a single removed row immediately
    // followed by a single added row, with nothing between them. Anything else - a moved
    // block, 2↔2, 1↔2 - stays line-level, which is the issue's rule and pi's.
    let mut spotlit: Vec<Option<String>> = vec![None; rows.len()];
    let mut index = 0;
    while index + 1 < rows.len() {
        if rows[index].kind == Kind::Removed
            && rows[index + 1].kind == Kind::Added
            && (index == 0 || rows[index - 1].kind != Kind::Removed)
            && (index + 2 >= rows.len() || rows[index + 2].kind != Kind::Added)
        {
            let (old, new) = spotlight(&rows[index].text, &rows[index + 1].text);
            spotlit[index] = Some(old);
            spotlit[index + 1] = Some(new);
            index += 2;
            continue;
        }
        index += 1;
    }

    let mut out = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        // The number the gutter shows: the new side for an addition, the old side for a
        // removal, and - for context, where the two are equal by definition - the new
        // side. A row before the first hunk header has neither and gets a blank gutter
        // rather than an invented number.
        let number = match row.kind {
            Kind::Added => row.new,
            Kind::Removed => row.old,
            Kind::Context => row.new.or(row.old),
        };
        let painted = match number {
            Some(n) => format!("{} {:>digits$} ", row.kind.marker(), n),
            None => format!("{} {} ", row.kind.marker(), " ".repeat(digits)),
        };
        let body = match &spotlit[index] {
            Some(text) => per_channel_only(&truncate_to_width(text, body_width, "…", false)),
            None => clip_plain(&row.text, body_width),
        };
        let gutter_styled = (theme.role(Role::Muted))(&painted);
        let body_styled = (theme.role(row.kind.role()))(&body);
        out.push(format!("  {gutter_styled}{body_styled}"));
    }
    out
}

#[cfg(test)]
#[path = "diff_card_tests.rs"]
mod tests;
