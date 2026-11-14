//! Main-screen (scrollback) renderer, ported from pi's
//! `packages/tui/src/tui-main-screen.ts`
//! (`pi-tui-re/src_re/tui-engine/tui-main-screen.md`).
//!
//! The UI renders into the terminal's main screen and scrollback, which is
//! why the transcript stays selectable and survives as history. The hot path
//! is a line diff: only the changed range is repainted, and growth appends
//! with real newlines so the terminal scrolls rather than overwrites.
//!
//! Porting notes from pi:
//! - Full document lines are passed to `render`, allowing real incremental appends
//!   and terminal scrollback accumulation.
//! - State tracks `previous_lines`, `previous_width`, `previous_height`, `cursor_row`,
//!   `hardware_cursor_row`, `max_lines_rendered`, and `previous_viewport_top`.
//! - `compute_line_diff` uses screen-row arithmetic against the moving viewport.
//! - When appending past the viewport bottom, newlines (`\r\n`) are emitted to scroll
//!   and `previous_viewport_top` moves forward.
//! - `append_start` distinguishes true append (begins with `\r\n`) from in-place edit (`\r`).
//! - Changed range is only first_changed..=last_changed.
//! - Width invariant defensively truncates rather than crashes.

use super::core::{SEGMENT_RESET, extract_cursor_position, width_violation};
use super::terminal::Terminal;
use super::text::truncate_to_width;
mod image_ranges;
use image_ranges::{delete_kitty_image, expand_range_for_images, orphaned_image_ids};

/// Whether this session is Termux, whose soft keyboard changes the terminal height by
/// 10-15 rows every time it shows or hides (gh #372 bug 3).
///
/// pi's `isTermuxSession` (`packages/tui/src/tui-main-screen.ts:109`): the whole rule is
/// a non-empty `TERMUX_VERSION`. Deliberately **not** a `TERM=screen` heuristic - tmux
/// and GNU screen also report `screen`, and they do not have a soft keyboard that
/// resizes the pane under the application.
pub fn is_termux_session() -> bool {
    is_termux_session_with(|key| std::env::var(key).ok())
}

/// The rule over an injected environment reader, so both answers are testable on one
/// machine (`lca-tui` denies `unsafe`, and a test cannot set the process environment).
fn is_termux_session_with(env: impl Fn(&str) -> Option<String>) -> bool {
    env("TERMUX_VERSION").is_some_and(|v| !v.is_empty())
}

/// The main-screen differential renderer.
#[derive(Debug, Default)]
pub struct MainScreenRenderer {
    previous_lines: Vec<String>,
    previous_width: u16,
    previous_height: u16,
    /// Logical index of the end of content.
    cursor_row: usize,
    /// Logical index of the hardware cursor.
    hardware_cursor_row: usize,
    /// High-water mark of rendered lines.
    max_lines_rendered: usize,
    /// Whether this is a Termux session (gh #372 bug 3): its soft keyboard resizes the
    /// terminal many times a second, so a height-only change must not rebuild.
    termux: bool,
    /// Clear emptied rows when the content shrinks below `max_lines_rendered`
    /// (gh #372 bug 4, pi's `terminal.clearOnShrink`).
    clear_on_shrink: bool,
    /// Logical top of the previous viewport.
    previous_viewport_top: usize,
    /// The caret from the last frame (row, col).
    last_cursor: Option<(u16, u16)>,
}

impl MainScreenRenderer {
    /// A fresh renderer.
    pub fn new() -> Self {
        Self {
            // Gh #372 bug 3: the Termux answer is a property of the session, decided
            // once here rather than re-read per frame. `set_termux_session` is the seam
            // a test drives both directions through.
            termux: is_termux_session(),
            ..Self::default()
        }
    }

    /// Declare this renderer's session Termux (gh #372 bug 3).
    pub fn set_termux_session(&mut self, on_termux: bool) {
        self.termux = on_termux;
    }

    /// Clear emptied rows when the content shrinks (gh #372 bug 4): pi's
    /// `setClearOnShrink`, the runtime half of the `terminal.clear_on_shrink` key.
    pub fn set_clear_on_shrink(&mut self, clear: bool) {
        self.clear_on_shrink = clear;
    }

    /// The lines from the last render.
    pub fn previous(&self) -> &[String] {
        &self.previous_lines
    }

    /// Reset internal render state (e.g. for full clear).
    pub fn reset_render_state(&mut self) {
        self.previous_lines.clear();
        self.previous_width = 0;
        self.previous_height = 0;
        self.cursor_row = 0;
        self.hardware_cursor_row = 0;
        self.max_lines_rendered = 0;
        self.previous_viewport_top = 0;
        self.last_cursor = None;
    }

    /// Render `lines` into the terminal, returning the hardware cursor
    /// position (row, col) if a `CURSOR_MARKER` was present.
    pub fn render(
        &mut self,
        term: &mut dyn Terminal,
        lines: Vec<String>,
        width: u16,
        height: u16,
    ) -> Option<(u16, u16)> {
        if width == 0 || height == 0 {
            return None;
        }

        let (mut new_lines, cursor_pos) = extract_cursor_position(&lines);

        // Width invariant: defensively truncate rather than crash.
        if let Some((_row, _w)) = width_violation(&new_lines, width) {
            for line in &mut new_lines {
                if super::text::visible_width(line) > width as usize {
                    *line = truncate_to_width(line, width as usize, "", false);
                }
            }
        }

        let width_changed = self.previous_width != 0 && self.previous_width != width;
        let height_changed = self.previous_height != 0 && self.previous_height != height;
        let prev_buf_len = if self.previous_height > 0 {
            self.previous_viewport_top + self.previous_height as usize
        } else {
            height as usize
        };
        let mut prev_viewport_top = if height_changed {
            prev_buf_len.saturating_sub(height as usize)
        } else {
            self.previous_viewport_top
        };
        let mut viewport_top = prev_viewport_top;
        let mut hardware_cursor_row = self.hardware_cursor_row;

        // Screen row of a document row: target - viewport_top
        // Line diff: target_screen_row - current_screen_row
        let compute_line_diff =
            |target_row: usize, hw_cursor: usize, p_view_top: usize, view_top: usize| -> i64 {
                let current_screen_row = hw_cursor as i64 - p_view_top as i64;
                let target_screen_row = target_row as i64 - view_top as i64;
                target_screen_row - current_screen_row
            };

        // Full render helper
        // Gh #324 item 1, measured: **a full clear must free every texture the
        // previous frame held.** The differential path diffs ids and deletes the
        // orphans (`deleted_ids` below), but every early return above (`width_changed`,
        // `height_changed`, `clear_on_shrink`, the shrink arm, and the first render)
        // reaches this helper and used to leave the old textures in the terminal with
        // nothing pointing at them. A width change is exactly the case that makes that
        // unbounded: the id hashes the cell scale, so **every** distinct width is a
        // distinct texture, and a resize drag at 30 columns wide mints 30 of them.
        //
        // pi's shape (`tui-main-screen.ts`'s `fullRender`): when the frame clears, it
        // emits `deleteKittyImages(previousKittyImageIds)` immediately **before**
        // `2J/H/3J`, inside the same synchronized block. All previous ids, not a diff:
        // the clear wipes the screen, so every texture the old frame referenced is
        // orphaned by definition. Emitting before the clear (rather than after) keeps
        // the ordering rule the differential path already states - the old texture never
        // shows for a frame after its replacement is on screen.
        let mut previous_ids: Vec<u32> = self
            .previous_lines
            .iter()
            .flat_map(|line| image_ranges::kitty_ids_on(line))
            .collect();
        previous_ids.sort_unstable();
        previous_ids.dedup();
        let mut full_render = |clear: bool, renderer: &mut Self| {
            let mut out = String::new();
            out.push_str("\x1b[?2026h"); // synchronized output
            if clear {
                for id in &previous_ids {
                    out.push_str(&delete_kitty_image(*id));
                }
                out.push_str("\x1b[2J\x1b[H\x1b[3J"); // Clear screen, home, clear scrollback
            }
            for (i, line) in new_lines.iter().enumerate() {
                if i > 0 {
                    out.push_str("\r\n");
                }
                out.push_str(line);
            }
            out.push_str(SEGMENT_RESET);
            out.push_str("\x1b[?2026l");
            term.write(&out);

            renderer.cursor_row = new_lines.len().saturating_sub(1);
            renderer.hardware_cursor_row = renderer.cursor_row;
            if clear {
                renderer.max_lines_rendered = new_lines.len();
            } else {
                renderer.max_lines_rendered = renderer.max_lines_rendered.max(new_lines.len());
            }
            let buffer_length = (height as usize).max(new_lines.len());
            renderer.previous_viewport_top = buffer_length.saturating_sub(height as usize);
            renderer.position_hardware_cursor(term, cursor_pos, new_lines.len(), height);
            renderer.previous_lines = new_lines.clone();
            renderer.previous_width = width;
            renderer.previous_height = height;
            renderer.last_cursor = cursor_pos.map(|(r, c, _)| (r, c));
        };

        // First render: output everything without clearing (assumes clean terminal/scrollback start)
        if self.previous_lines.is_empty() && !width_changed && !height_changed {
            full_render(false, self);
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        // Width changes invalidate wrapping: full clear + re-render
        if width_changed {
            full_render(true, self);
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        // A height change needs a full redraw to re-align the visible viewport - except
        // on Termux (gh #372 bug 3), where the soft keyboard changes the height by
        // 10-15 rows every time it shows or hides. A full render there is
        // `2J/H/3J`, which wipes the screen **and the scrollback** on every toggle, so
        // the transcript the user was reading disappears repeatedly.
        //
        // The exemption is **height-only**: `width_changed` returned above, because a
        // rotation or a genuine resize does invalidate wrapping and must rebuild.
        //
        // `shortcut:` the exemption covers the height change itself, not the
        // deleted-lines arm further down - if a soft-keyboard toggle coincides with
        // content *shrinking* below the new viewport top, that arm still calls
        // `full_render`. pi has the identical hole (`tui-main-screen.ts`, the
        // `targetRow < prevViewportTop` branch), so closing it means changing both the
        // scroll arithmetic and this exemption together; upgrade when a Termux report
        // shows it, rather than widening the exemption blind.
        if height_changed && !self.termux {
            full_render(true, self);
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        // Gh #372 bug 4: the content shrank below what was last painted, so rows that
        // used to carry text are now empty on screen. pi's `terminal.clearOnShrink`
        // (`tui-main-screen.ts:357`) re-renders to clear them - and only when no
        // overlay is open, because an overlay needs the padding that a cleared frame
        // would remove. Off by default: the flag is the user's, not a guess.
        if self.clear_on_shrink && new_lines.len() < self.max_lines_rendered {
            full_render(true, self);
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        // Find first and last changed lines
        let mut first_changed: Option<usize> = None;
        let mut last_changed: Option<usize> = None;
        let max_lines = new_lines.len().max(self.previous_lines.len());
        for i in 0..max_lines {
            let old_line = self.previous_lines.get(i).map(String::as_str).unwrap_or("");
            let new_line = new_lines.get(i).map(String::as_str).unwrap_or("");
            if old_line != new_line {
                if first_changed.is_none() {
                    first_changed = Some(i);
                }
                last_changed = Some(i);
            }
        }

        let appended_lines = new_lines.len() > self.previous_lines.len();
        if appended_lines {
            if first_changed.is_none() {
                first_changed = Some(self.previous_lines.len());
            }
            last_changed = Some(new_lines.len() - 1);
        }

        // Gh #324 gap 4: the textures this repaint orphans. Computed here, emitted
        // inside the frame below - `full_render` holds a mutable borrow of `term`, so the
        // write has to happen where the other frame writes do.
        //
        // A *replaced* image leaves its old texture in the terminal with nothing pointing
        // at it; over a session that accumulates one per replacement. A *moved* one
        // (gap 3's crop, same id, new placement) must **not** be deleted - deleting it
        // would remove the texture the next line is about to place, which is gap 3
        // undone. So this is an id **diff**, not "delete anything in range".
        let deleted_ids = orphaned_image_ids(&self.previous_lines, &new_lines);

        // Gh #324 gap 4: **make the range image-aware before it is used.** An interval
        // that starts or ends inside a multi-row image cannot be repainted: the terminal
        // has no way to redraw part of a texture, so the untouched half keeps the old
        // drawing and the repainted half shows the new frame. This has to happen here,
        // upstream of every arm that consumes the range, because each of them computes
        // its cursor moves from it.
        let (first_changed, last_changed) = match (first_changed, last_changed) {
            (Some(first), Some(last)) => {
                let (first, last) =
                    expand_range_for_images(&self.previous_lines, &new_lines, first, last);
                (Some(first), Some(last))
            }
            other => other,
        };

        let append_start = appended_lines
            && first_changed == Some(self.previous_lines.len())
            && !self.previous_lines.is_empty();

        // Caret move check
        let cursor_moved = cursor_pos.map(|(r, c, _)| (r, c)) != self.last_cursor;

        // No text changes
        if first_changed.is_none() {
            if cursor_moved && let Some((row, _, _)) = cursor_pos {
                // If only the caret moved, repaint its row to ensure visible caret update
                let doc_row = (row as usize).min(new_lines.len().saturating_sub(1));
                // Gh #371 bug 1: repainting a row is a screen operation, so it only
                // exists for a row that is on screen. A caret above the viewport sits
                // in scrollback the terminal cannot address, so there is nothing to
                // repaint and nothing to record - fall through to
                // `position_hardware_cursor`, which hides the cursor for an
                // unaddressable caret.
                let on_screen = doc_row >= prev_viewport_top
                    && doc_row < prev_viewport_top.saturating_add(height as usize);
                if on_screen {
                    let line_diff = compute_line_diff(
                        doc_row,
                        hardware_cursor_row,
                        prev_viewport_top,
                        viewport_top,
                    );
                    let mut out = String::new();
                    out.push_str("\x1b[?2026h");
                    if line_diff > 0 {
                        out.push_str(&format!("\x1b[{}B", line_diff));
                    } else if line_diff < 0 {
                        out.push_str(&format!("\x1b[{}A", -line_diff));
                    }
                    out.push_str("\r\x1b[2K");
                    out.push_str(&new_lines[doc_row]);
                    out.push_str(SEGMENT_RESET);
                    out.push_str("\x1b[?2026l");
                    term.write(&out);
                    self.hardware_cursor_row = doc_row;
                }
            }
            self.position_hardware_cursor(term, cursor_pos, new_lines.len(), height);
            self.previous_viewport_top = prev_viewport_top;
            self.previous_height = height;
            self.last_cursor = cursor_pos.map(|(r, c, _)| (r, c));
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        let first_changed_idx = first_changed.unwrap_or(0);
        let last_changed_idx = last_changed.unwrap_or(first_changed_idx);

        // All changes are in deleted lines (nothing to render, just clear trailing)
        if first_changed_idx >= new_lines.len() {
            if self.previous_lines.len() > new_lines.len() {
                let target_row = new_lines.len().saturating_sub(1);
                if target_row < prev_viewport_top {
                    full_render(true, self);
                    return cursor_pos.map(|(r, c, _)| (r, c));
                }
                let line_diff = compute_line_diff(
                    target_row,
                    hardware_cursor_row,
                    prev_viewport_top,
                    viewport_top,
                );
                let mut out = String::new();
                out.push_str("\x1b[?2026h");
                if line_diff > 0 {
                    out.push_str(&format!("\x1b[{}B", line_diff));
                } else if line_diff < 0 {
                    out.push_str(&format!("\x1b[{}A", -line_diff));
                }
                out.push('\r');
                let extra_lines = self.previous_lines.len() - new_lines.len();
                if extra_lines > height as usize {
                    full_render(true, self);
                    return cursor_pos.map(|(r, c, _)| (r, c));
                }
                let clear_start_offset = if new_lines.is_empty() { 0 } else { 1 };
                if extra_lines > 0 && clear_start_offset > 0 {
                    out.push_str(&format!("\x1b[{}B", clear_start_offset));
                }
                for i in 0..extra_lines {
                    out.push_str("\r\x1b[2K");
                    if i + 1 < extra_lines {
                        out.push_str("\x1b[1B");
                    }
                }
                let move_back = extra_lines.saturating_sub(1) + clear_start_offset;
                if move_back > 0 {
                    out.push_str(&format!("\x1b[{}A", move_back));
                }
                out.push_str("\x1b[?2026l");
                term.write(&out);
                self.cursor_row = target_row;
                self.hardware_cursor_row = target_row;
            }
            self.position_hardware_cursor(term, cursor_pos, new_lines.len(), height);
            self.previous_lines = new_lines;
            self.previous_width = width;
            self.previous_height = height;
            self.previous_viewport_top = prev_viewport_top;
            self.last_cursor = cursor_pos.map(|(r, c, _)| (r, c));
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        // Differential rendering can only touch what was actually visible on the terminal screen.
        // If the first changed line is above the visible viewport, fall back to a full redraw.
        if first_changed_idx < prev_viewport_top {
            full_render(true, self);
            return cursor_pos.map(|(r, c, _)| (r, c));
        }

        let mut out = String::new();
        out.push_str("\x1b[?2026h");

        // Gh #324 gap 4: the orphaned textures, deleted **inside** this frame's
        // synchronized block so the terminal applies the delete and the repaint together.
        // Deleting before the block would flicker: the old image would vanish for a
        // frame before the new drawing arrived.
        for id in &deleted_ids {
            out.push_str(&delete_kitty_image(*id));
        }

        let prev_viewport_bottom = prev_viewport_top + height as usize - 1;
        let move_target_row = if append_start {
            first_changed_idx.saturating_sub(1)
        } else {
            first_changed_idx
        };

        if move_target_row > prev_viewport_bottom {
            let current_screen_row =
                ((hardware_cursor_row as i64 - prev_viewport_top as i64).max(0) as usize)
                    .min(height.saturating_sub(1) as usize);
            let move_to_bottom = (height as usize).saturating_sub(1 + current_screen_row);
            if move_to_bottom > 0 {
                out.push_str(&format!("\x1b[{}B", move_to_bottom));
            }
            let scroll = move_target_row - prev_viewport_bottom;
            for _ in 0..scroll {
                out.push_str("\r\n");
            }
            prev_viewport_top += scroll;
            viewport_top += scroll;
            hardware_cursor_row = move_target_row;
        }

        let line_diff = compute_line_diff(
            move_target_row,
            hardware_cursor_row,
            prev_viewport_top,
            viewport_top,
        );
        if line_diff > 0 {
            out.push_str(&format!("\x1b[{}B", line_diff));
        } else if line_diff < 0 {
            out.push_str(&format!("\x1b[{}A", -line_diff));
        }

        let render_end = last_changed_idx.min(new_lines.len().saturating_sub(1));
        // gh #18 (LCA-PROMPT-GH18FIX2): clear every row this frame rewrites
        // before painting any of them. The shared baseline - pi's own diff
        // loop, the `else` branch of `tui-alt-screen.ts`' redraw - paints
        // `2K` + text per row as it walks down, which is correct the moment
        // `?2026` is honored; a terminal or capture path that ignores
        // synchronized output walks it live and can show a row of the
        // outgoing frame beside a row of the incoming one - the report's
        // stale notice. Same rows, same clears, same text, only the order.
        //
        // The clear pass walks with `\x1b[nB`/`\x1b[nA` rather than the
        // content walk's `\r\n`: cursor-movement sequences never scroll,
        // so the append/scroll contract below (FR-UI-24, gh #33) sees the
        // same newlines it saw before. One rewritten row and pure appends
        // keep their existing order - nothing to interleave with, and the
        // pinned `\r\n\x1b[2K`/`\r\x1b[2K` shapes of the append and
        // caret rows depend on it.
        // Only rows the pane can actually show: a clear pass may not walk
        // past the viewport with `\x1b[nB`, because CUD clamps at the last
        // screen row while the matching `\x1b[nA` does not - the cursor
        // would come back above the range and the content walk would paint
        // off its rows. Rows below the viewport are the trailing block's
        // to clear, exactly as before.
        let visible_last = viewport_top.saturating_add((height as usize).saturating_sub(1));
        // Exactly the rows this frame touches: `first..=last_changed`
        // covers both the rewritten rows and the vacated tail (when the
        // frame shrinks, `last_changed` runs past the new end). Ending at
        // `render_end.max(previous.len() - 1)` instead would clear rows
        // beyond `last_changed` that the two frames agree on - unchanged
        // content erased and never repainted, which is how this row's
        // status line vanished in the first attempt.
        let clear_end = last_changed_idx;
        let clear_last = clear_end.min(visible_last);
        let rewrite_rows = clear_last.saturating_sub(first_changed_idx) + 1;
        let clear_first = !append_start && rewrite_rows >= 2;

        if append_start {
            out.push_str("\r\n");
        } else {
            out.push('\r');
        }

        if clear_first {
            for offset in 0..rewrite_rows {
                if offset > 0 {
                    out.push_str("\x1b[1B");
                }
                out.push_str("\x1b[2K");
            }
            if rewrite_rows > 1 {
                out.push_str(&format!("\x1b[{}A", rewrite_rows - 1));
            }
        }

        // Gh #324 gap 1: an image's reserved rows are **one block**, and the
        // paint loop must not walk into it. The old loop wrote `\r\n` for every
        // row including the blanks below a kitty header, so each blank took the
        // text path: the cursor left the block's top, the reserve was never
        // claimed, and the transcript row after the image landed on it. pi's
        // `getKittyImageReservedRows` is the same shape - walk the reserve, then
        // come back up - and the `i += ahead` below is what makes a blank row
        // inside a reserve unreachable from the text path.
        let mut i = first_changed_idx;
        while i <= render_end {
            let line = &new_lines[i];
            // Rows still inside a reserve that started above this frame's first
            // changed row belong to that image, not to this repaint.
            if line.is_empty() {
                if i > first_changed_idx {
                    out.push_str("\r\n");
                }
                if !clear_first {
                    out.push_str("\x1b[2K");
                }
                out.push_str(line);
                i += 1;
                continue;
            }
            let reserved = crate::widgets::image::kitty_block_rows(&new_lines, i)
                .unwrap_or(1)
                .max(1) as usize;
            // How many of those rows are still ahead of us in this walk.
            let ahead = reserved.min(render_end.saturating_sub(i) + 1);
            if i > first_changed_idx {
                out.push_str("\r\n");
            }
            if ahead > 1 {
                if !clear_first {
                    out.push_str("\x1b[2K");
                }
                for row in 1..ahead {
                    let blank = new_lines.get(i + row);
                    debug_assert!(
                        blank.is_none_or(|b| b.is_empty()),
                        "a reserved row below the image header is blank"
                    );
                    out.push_str("\r\n\x1b[2K");
                }
                out.push_str(&format!("\x1b[{}A", ahead - 1));
                out.push_str(line);
                out.push_str(&format!("\x1b[{}B", ahead - 1));
                i += ahead;
                continue;
            }
            if !clear_first {
                out.push_str("\x1b[2K");
            }
            out.push_str(line);
            i += 1;
        }
        out.push_str(SEGMENT_RESET);

        let mut final_cursor_row = render_end;

        if self.previous_lines.len() > new_lines.len() {
            if render_end < new_lines.len().saturating_sub(1) {
                let move_down = new_lines.len() - 1 - render_end;
                out.push_str(&format!("\x1b[{}B", move_down));
                final_cursor_row = new_lines.len() - 1;
            }
            let extra_lines = self.previous_lines.len() - new_lines.len();
            // Rows the clear pass already emptied are not cleared twice;
            // the ones below the viewport keep this walk, which is the only
            // thing that can reach them. The cursor's net move is zero
            // either way, so the bookkeeping below is unchanged.
            let covered = if clear_first {
                clear_last.saturating_sub(render_end)
            } else {
                0
            };
            let moves = extra_lines.saturating_sub(covered);
            for _ in 0..moves {
                out.push_str("\r\n\x1b[2K");
            }
            out.push_str(&format!("\x1b[{}A", moves));
        }

        out.push_str("\x1b[?2026l"); // end synchronized output
        term.write(&out);

        self.cursor_row = new_lines.len().saturating_sub(1);
        self.hardware_cursor_row = final_cursor_row;
        self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len());
        self.previous_viewport_top =
            prev_viewport_top.max((final_cursor_row + 1).saturating_sub(height as usize));

        self.position_hardware_cursor(term, cursor_pos, new_lines.len(), height);

        self.previous_lines = new_lines;
        self.previous_width = width;
        self.previous_height = height;
        self.last_cursor = cursor_pos.map(|(r, c, _)| (r, c));
        cursor_pos.map(|(r, c, _)| (r, c))
    }

    /// Position the hardware cursor for IME candidate window or caret tracking.
    fn position_hardware_cursor(
        &mut self,
        term: &mut dyn Terminal,
        cursor: Option<(u16, u16, bool)>,
        total_lines: usize,
        height: u16,
    ) {
        if total_lines == 0 {
            term.hide_cursor();
            return;
        }

        let Some((row, col, painted)) = cursor else {
            term.hide_cursor();
            return;
        };

        // Gh #371 bug 1: the recorded row must be a row the terminal can be on.
        //
        // The defect was not a sign error - it was recording the wrong fact. We asked
        // for `target_row`, the emulator clamped the *result* at the viewport top, and
        // this function then stored the row it had asked for. The next frame's
        // `compute_line_diff` therefore started from a screen row that never existed
        // (`hw_cursor - p_view_top < 0`) and the differential rendering desynchronised
        // permanently.
        //
        // A caret above the viewport is not a cursor position at all: the line it sits
        // on has scrolled into the terminal's scrollback, which the hardware cursor
        // cannot address. So hide it and leave `hardware_cursor_row` alone rather than
        // "clamping and pretending" - clamping would park the IME candidate window on
        // an unrelated visible line, which is worse than anchoring nothing. The painted
        // caret still shows the caret's real position in the transcript, which is what
        // a reader uses (ADR-0037: the painted caret is authoritative).
        let viewport_top = self.previous_viewport_top;
        let viewport_bottom = viewport_top.saturating_add(height as usize).max(1);
        if (row as usize) < viewport_top || (row as usize) >= viewport_bottom {
            term.hide_cursor();
            return;
        }

        let target_row = (row as usize).min(total_lines.saturating_sub(1));
        let row_delta = target_row as i64 - self.hardware_cursor_row as i64;
        let mut buffer = String::new();
        if row_delta > 0 {
            buffer.push_str(&format!("\x1b[{}B", row_delta));
        } else if row_delta < 0 {
            buffer.push_str(&format!("\x1b[{}A", -row_delta));
        }
        buffer.push_str(&format!("\x1b[{}G", col + 1));
        if painted {
            buffer.push_str("\x1b[?25l");
        } else {
            buffer.push_str("\x1b[?25h");
        }
        term.write(&buffer);
        self.hardware_cursor_row = target_row;
    }

    /// Park the cursor below the content so the shell prompt lands cleanly.
    ///
    /// The hardware cursor rests wherever the last frame left it - usually
    /// the caret, in the editor line - and the document's final rows (the
    /// footer) sit below that, so the descent comes first and the newline
    /// alone would park the prompt on the footer (gh #33).
    pub fn finish(&mut self, term: &mut dyn Terminal) {
        let mut out = String::new();
        let down = self.cursor_row.saturating_sub(self.hardware_cursor_row);
        if down > 0 {
            out.push_str(&format!("\x1b[{down}B"));
        }
        out.push_str("\r\n\x1b[?7h\x1b[?25h");
        term.write(&out);
    }
}

#[cfg(test)]
#[path = "main_screen/image_range_tests.rs"]
mod image_range_tests;
#[cfg(test)]
#[path = "main_screen/image_reserve_tests.rs"]
mod image_reserve_tests;
#[cfg(test)]
#[path = "main_screen/tests.rs"]
mod tests;
