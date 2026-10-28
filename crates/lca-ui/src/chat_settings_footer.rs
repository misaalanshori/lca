//! The `/settings` footer (gh #386 gap 3): the highlighted row's description, whole.
//!
//! Gap 1 gave every settings row a description *column*, so the sentence on the row
//! is truncated to fit it. The footer is where the rest of the sentence goes - it
//! has the box's whole inner width to wrap in, and it is the piece of pi's
//! `SettingsList` (`settings-list.ts::renderMainList`) that LCA was missing.
//!
//! Split from `chat_overlays.rs` under gate 11's 1,200-line ceiling, and split along
//! a real seam: this is the only place that decides what a footer row *is*, while the
//! overlays file decides where it goes on screen.

/// How many cells a `/settings` footer may paint in.
///
/// pi wraps the description to `width - 4` (`settings-list.ts`), and the box has
/// already taken two border cells and two padding cells per side by the time its
/// inner width is known - so `width - 4` is the same number the rows use, which is
/// what keeps the footer's text inside the frame.
pub(crate) fn settings_footer_width(term_width: u16) -> usize {
    (term_width as usize).saturating_sub(4).max(1)
}

/// How many rows a `/settings` footer is allowed to take.
///
/// A cap, because the footer must not become the screen: a long description that
/// wrapped to twenty rows would leave one item visible, which is the "pushes the list
/// off the pane" failure in its other form. pi has no cap (its descriptions are one
/// sentence each); two rows holds the longest description in `/settings` at 80
/// columns, and a description needing a third belongs in documentation.
pub(crate) const SETTINGS_FOOTER_MAX_ROWS: usize = 2;

/// The `/settings` footer's rows for the highlighted row: a blank separator, then the
/// row's description wrapped to `width` (gh #386 gap 3).
///
/// Empty when the row has no description - a blank separator with nothing after it
/// would be two rows of air where the list could be. The `SETTINGS_FOOTER_MAX_ROWS`
/// cap is applied **after** wrapping and elides with `…`, so a clipped footer says so
/// rather than ending mid-word.
pub(crate) fn settings_footer(row: Option<&crate::state::SettingRow>, width: usize) -> Vec<String> {
    let Some(description) = row.map(|row| row.description.as_str()) else {
        return Vec::new();
    };
    if description.is_empty() {
        return Vec::new();
    }
    let mut wrapped = lca_tui::engine::text::wrap_text_with_ansi(description, width);
    if wrapped.len() > SETTINGS_FOOTER_MAX_ROWS {
        // Say it was cut. Truncating is not enough on its own: a wrapped line is
        // already at most `width`, so cutting it to `width - 1` leaves a string that
        // fits and `truncate_to_width` returns unchanged - the ellipsis never appears.
        // Appending and re-cutting is what actually marks the elision.
        wrapped.truncate(SETTINGS_FOOTER_MAX_ROWS);
        if let Some(last) = wrapped.last_mut() {
            last.push('\u{2026}');
            *last = lca_tui::engine::text::truncate_to_width(last, width, "", true);
        }
    }
    let mut rows = vec![String::new()];
    rows.extend(wrapped);
    rows
}

/// The footer rows for the settings picker, or none when the row is being edited.
///
/// The one call site's form of [`settings_footer`], including the two decisions that
/// belong with the footer rather than with the painter: an editing row has its buffer
/// on the row itself, so a description below it would compete with the field being
/// typed into; and the width comes from [`settings_footer_width`], not from anything
/// the caller measures.
pub(crate) fn settings_footer_for(
    picker: &crate::chat_pickers::SettingsPicker,
    term_width: u16,
) -> Vec<String> {
    if picker.editing.is_some() {
        return Vec::new();
    }
    settings_footer(picker.selected_row(), settings_footer_width(term_width))
}

/// Whether the box on this pane has room for `footer` **without taking an item row**.
///
/// The footer exists to show a sentence the row's description column had to cut, so
/// it must never be paid for out of the list. This asks the box's own layout rule
/// whether the height it would need is the height it would get: if the resolved
/// height equals the requested one, the rows cost nothing; if it is smaller, the
/// terminal is clamping the box and the footer would have come out of the items.
///
/// Asking the rule rather than re-deriving the grant is deliberate - a second copy of
/// "how tall may this box be" is how the painter and the hit test would drift apart.
pub(crate) fn picker_footer_fits(
    body_len: usize,
    footer_len: usize,
    term_width: u16,
    term_height: u16,
    above_rows: usize,
) -> bool {
    if footer_len == 0 {
        return false;
    }
    let options = crate::render::picker_overlay_options(term_height, above_rows);
    // The body already carries the two pinned hint rows, so the box wants its height
    // plus two frame rows plus the footer.
    let without = u16::try_from(body_len)
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let requested = without.saturating_add(u16::try_from(footer_len).unwrap_or(u16::MAX));
    let resolved =
        lca_tui::engine::core::resolve_overlay_layout(&options, term_width, term_height, requested);
    resolved.height >= requested
}

/// The `/settings` hint line with the description folded into it, for a box with no
/// room for a real footer (gh #235, gh #386 gap 3).
///
/// This is the **fallback**, and it is why the footer can safely yield to the list: on
/// a tall list the box is clamped by the terminal, so a wrapped footer would have to
/// be paid for out of the items. Rather than lose the help entirely - which the first
/// version of the footer did, and a live row caught - the description goes back on the
/// hint line, truncated, exactly as it was before this gap.
///
/// A prose description that fits in one line keeps the whole sentence; a longer one is
/// cut at `cap` with an ellipsis, because the hint line shares its row with the keys.
pub(crate) fn settings_hint_with_description(
    picker: &crate::chat_pickers::SettingsPicker,
    term_width: u16,
) -> String {
    const HINT: &str =
        "↑↓ move · enter/space/←→ change · tab project · q/esc close · type to search";
    let Some(row) = picker.selected_row() else {
        return HINT.to_string();
    };
    let cap = (term_width as usize)
        .saturating_sub(HINT.len() + 12)
        .max(20);
    let description =
        lca_tui::engine::text::truncate_to_width(&row.description, cap, "\u{2026}", false);
    if description.is_empty() {
        HINT.to_string()
    } else {
        format!("{HINT} · {description}")
    }
}

/// The `/settings` overlay's footer rows **and** hint line, decided together (gh #386
/// gap 3).
///
/// They are one decision, not two: a footer that fits means the hint is the plain
/// key summary, and a footer that does not means the description moves *onto* the hint
/// line. Returning them from one function is what stops the two from disagreeing -
/// separately computed, a "footer fits" answer with an "also show it on the hint"
/// answer would paint the sentence twice, which is exactly the double-insert this gap
/// already produced once.
#[allow(clippy::too_many_arguments)]
pub(crate) fn settings_footer_and_hint(
    picker: &crate::chat_pickers::SettingsPicker,
    term_width: u16,
    term_height: u16,
    body_len: usize,
    above_rows: usize,
    edit_hint: &str,
    search_hint: &str,
) -> (Vec<String>, String) {
    if picker.editing.is_some() {
        // An editing row has its buffer on the row itself, so a description below it
        // would compete with the field being typed into.
        return (Vec::new(), edit_hint.to_string());
    }
    let footer = settings_footer_for(picker, term_width);
    // The painter adds the two frame rows; `body_len` is what it will hold, hint rows
    // included.
    let fits = picker_footer_fits(
        body_len + 2,
        footer.len(),
        term_width,
        term_height,
        above_rows,
    );
    if fits {
        (footer, search_hint.to_string())
    } else {
        let hint = settings_hint_with_description(picker, term_width);
        // The no-description case still wants the caller's own hint spelling.
        if hint == search_hint || picker.selected_row().is_none() {
            (Vec::new(), search_hint.to_string())
        } else {
            (Vec::new(), hint)
        }
    }
}
