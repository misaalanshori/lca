//! File-edit matching: BOM/line-ending normalization and the fuzzy
//! fallback (gh #114, gh #115).
//!
//! Ported from pi's `edit-diff.ts` (`detectLineEnding`,
//! `normalizeToLF`, `restoreLineEndings`, `normalizeForFuzzyMatch`,
//! `fuzzyFindText`, `applyEditsToNormalizedContent`), adapted to this
//! tool's error contract (the model's messages stay ours; only the
//! matching semantics converge). Pipeline order is pi's: strip the BOM,
//! normalize to LF for matching, then restore the original ending style
//! plus BOM on write. Fuzzy normalization runs after line-ending
//! normalization, never before.
//!
//! The fuzzy path never wins over an exact match (exact first, always),
//! and several fuzzy hits are an ambiguity error, not a guess. When any
//! edit matches fuzzily, replacements run in normalized space and are
//! overlaid onto the original line-wise, so untouched lines keep their
//! exact bytes (trailing spaces, curly quotes and all).

use unicode_normalization::UnicodeNormalization;

use std::path::Path;

use crate::{ToolCall, ToolResult, resolve_target};

impl crate::ToolExecutor {
    pub(crate) async fn edit(&mut self, call: &ToolCall, args: &serde_json::Value) -> ToolResult {
        let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
            return ToolResult::error(call.call_id.clone(), "path is required");
        };
        // EFG-014: pi's legacy input - a single top-level
        // `{oldText,newText}` applies like a one-element `edits` array
        // (models trained on pi emit it), normalized here at parse time.
        // An `edits` array that has entries wins; the validation below
        // and every edit/replace rule are untouched.
        let edits: Vec<serde_json::Value> = match args.get("edits").and_then(|v| v.as_array()) {
            Some(array) if !array.is_empty() => array.clone(),
            _ => match (
                args.get("oldText").and_then(|v| v.as_str()),
                args.get("newText").and_then(|v| v.as_str()),
            ) {
                (Some(old), Some(new)) => {
                    vec![serde_json::json!({ "oldText": old, "newText": new })]
                }
                _ => {
                    return ToolResult::error(
                        call.call_id.clone(),
                        "edits must contain at least one replacement",
                    );
                }
            },
        };
        let target = resolve_target(&self.cwd, Path::new(path));
        let original = match self.ops.read(&target) {
            Ok(bytes) => bytes,
            Err(err) => {
                return ToolResult::error(
                    call.call_id.clone(),
                    format!("Could not edit file: {path}. {err}."),
                );
            }
        };
        // FR-TOOL-2: reject when the file changed since this session last
        // read it (or was never read at all) - unless the host turned
        // the gate off for pi parity (gh #117).
        if self.edit_requires_read && !self.tracker.fresh_read(&target, &original) {
            return ToolResult::error(
                call.call_id.clone(),
                format!(
                    "The file {path} changed since it was last read (or was never read this session). \
                     Read it again before editing."
                ),
            );
        }
        // #116: strict decode at the entry. Lossy decoding here would
        // write U+FFFD over bytes outside the edited region — silent
        // corruption of Latin-1/Shift-JIS files. Refuse instead; the
        // file's bytes stay exactly as they were.
        let original = match String::from_utf8(original) {
            Ok(text) => text,
            Err(_) => {
                return ToolResult::error(
                    call.call_id.clone(),
                    format!(
                        "cannot edit {path}: the file is not valid UTF-8; editing it would corrupt the undecodable bytes, so the file was left unchanged"
                    ),
                );
            }
        };

        // The match pipeline (gh #114, gh #115, pi's order): strip the
        // BOM the model never sends, normalize to LF for matching, run
        // every edit against that one view (exact first, fuzzy fallback),
        // then restore the file's own ending style plus BOM on write.
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(edits.len());
        for (index, edit) in edits.iter().enumerate() {
            let (Some(old), Some(new)) = (
                edit.get("oldText").and_then(|v| v.as_str()),
                edit.get("newText").and_then(|v| v.as_str()),
            ) else {
                return ToolResult::error(
                    call.call_id.clone(),
                    format!("edits[{index}] needs oldText and newText"),
                );
            };
            pairs.push((
                crate::edit::normalize_to_lf(old),
                crate::edit::normalize_to_lf(new),
            ));
        }
        let (bom, content) = crate::edit::strip_bom(&original);
        let ending = crate::edit::detect_line_ending(content);
        let normalized = crate::edit::normalize_to_lf(content);
        let replaced = match crate::edit::apply_edits(&normalized, &pairs, path) {
            Ok(text) => text,
            Err(err) => return ToolResult::error(call.call_id.clone(), err),
        };
        let mut out = crate::edit::restore_line_endings(&replaced, ending);
        if bom {
            out.insert(0, '\u{FEFF}');
        }
        match self.ops.write(&target, out.as_bytes()) {
            Ok(()) => {
                self.tracker.record(&target, out.as_bytes());
                let mut result = ToolResult::ok(
                    call.call_id.clone(),
                    format!("Successfully replaced {} block(s) in {path}.", pairs.len()),
                );
                // EFG-014: the diff rides as structured data, computed
                // from the before/after this call already holds - display
                // only, never a claim: a change that did not happen
                // produces no diff and no `diff` entry. Diffed in LF
                // space, so a CRLF file does not paint `\r` noise.
                let diff = crate::diff::unified_diff(&normalized, &replaced, path);
                if !diff.is_empty() {
                    result.extras.insert("diff".to_string(), diff);
                }
                result
            }
            Err(err) => {
                ToolResult::error(call.call_id.clone(), format!("cannot write {path}: {err}"))
            }
        }
    }
}

/// The file's line-ending style, for restoring after the match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    /// `\r\n` throughout (pi: the first newline in the file decides).
    Crlf,
    /// `\n` (also the empty file and the no-newline file).
    Lf,
}

/// Split a leading UTF-8 BOM: whether one was present, and the text
/// without it. The model never includes the invisible BOM in `oldText`,
/// so matching runs without it and the write restores it.
pub fn strip_bom(text: &str) -> (bool, &str) {
    match text.strip_prefix('\u{FEFF}') {
        Some(rest) => (true, rest),
        None => (false, text),
    }
}

/// pi's `detectLineEnding`: the first newline in the file decides.
/// A lone `\r` never sets the style (it normalizes to LF either way).
pub fn detect_line_ending(content: &str) -> LineEnding {
    let lf = content.find('\n');
    let crlf = content.find("\r\n");
    match (lf, crlf) {
        (None, _) | (_, None) => LineEnding::Lf,
        (Some(lf), Some(crlf)) if crlf < lf => LineEnding::Crlf,
        _ => LineEnding::Lf,
    }
}

/// pi's `normalizeToLF`: CRLF pairs, then any lone CR left over.
pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// pi's `restoreLineEndings`: LF back to the file's own style.
pub fn restore_line_endings(text: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::Lf => text.to_string(),
        LineEnding::Crlf => text.replace('\n', "\r\n"),
    }
}

/// pi's `normalizeForFuzzyMatch`: NFKC, trailing whitespace stripped per
/// line, smart quotes to ASCII, Unicode dashes to `-`, special spaces to
/// a plain space. Line count is preserved (split and rejoin on `\n`),
/// which the overlay relies on.
pub fn normalize_for_fuzzy_match(text: &str) -> String {
    text.nfkc()
        .collect::<String>()
        .split('\n')
        .map(|line| {
            line.trim_end()
                .chars()
                .map(|c| match c {
                    '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
                    '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
                    '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
                    | '\u{2212}' => '-',
                    '\u{A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
                    | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}'
                    | '\u{205F}' | '\u{3000}' => ' ',
                    other => other,
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One match of `oldText` against content already in LF space.
struct Hit {
    /// Byte offset in the space that was searched.
    index: usize,
    /// Matched length in that space.
    length: usize,
    /// Whether the fuzzy normalization found it.
    fuzzy: bool,
}

/// pi's `fuzzyFindText`: exact match first, always; the fuzzy fallback
/// searches normalized content for normalized `oldText`.
fn fuzzy_find(content: &str, old: &str) -> Option<Hit> {
    if let Some(index) = content.find(old) {
        return Some(Hit {
            index,
            length: old.len(),
            fuzzy: false,
        });
    }
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old = normalize_for_fuzzy_match(old);
    fuzzy_content.find(&fuzzy_old).map(|index| Hit {
        index,
        length: fuzzy_old.len(),
        fuzzy: true,
    })
}

/// Occurrence count, always in normalized space (pi's `countOccurrences`
/// normalizes unconditionally): one raw hit beside one fuzzy-only hit is
/// still two, and still ambiguous. Exact-first decides *which* hit wins;
/// counting decides whether the win was unique.
fn count_occurrences(content: &str, old: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old = normalize_for_fuzzy_match(old);
    if fuzzy_old.is_empty() {
        return 0;
    }
    fuzzy_content.match_indices(&fuzzy_old).count()
}

/// A matched edit: where it landed (in replacement-base space) and what
/// goes there.
struct Matched {
    /// Position in the caller's `edits` array (error messages).
    edit_index: usize,
    /// Byte offset in the replacement base.
    index: usize,
    /// Matched length in the replacement base.
    length: usize,
    /// The new text (LF-normalized).
    new_text: String,
}

/// Match every edit against the same LF content: exact first, fuzzy
/// fallback, uniqueness in the matched space, no overlaps. Returns the
/// matched edits plus whether any of them went fuzzy (which decides the
/// replacement space).
fn match_edits(
    content: &str,
    edits: &[(String, String)],
    path: &str,
) -> Result<(Vec<Matched>, bool), String> {
    let mut matched = Vec::with_capacity(edits.len());
    let mut any_fuzzy = false;
    for (i, (old, new)) in edits.iter().enumerate() {
        if old.is_empty() {
            return Err(format!("edits[{i}].oldText must not be empty in {path}"));
        }
        let Some(hit) = fuzzy_find(content, old) else {
            return Err(format!("edits[{i}].oldText not found in {path}"));
        };
        any_fuzzy |= hit.fuzzy;
        if count_occurrences(content, old) > 1 {
            return Err(format!(
                "edits[{i}].oldText matches more than once in {path}; make it unique"
            ));
        }
        matched.push(Matched {
            edit_index: i,
            index: hit.index,
            length: hit.length,
            new_text: new.clone(),
        });
    }
    matched.sort_by_key(|m| m.index);
    for pair in matched.windows(2) {
        if pair[0].index + pair[0].length > pair[1].index {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {path}; merge overlapping changes into one edit",
                pair[0].edit_index, pair[1].edit_index
            ));
        }
    }
    Ok((matched, any_fuzzy))
}

/// Split keeping line endings, pi's `splitLinesWithEndings`.
fn split_lines(body: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, c) in body.char_indices() {
        if c == '\n' {
            lines.push(&body[start..=i]);
            start = i + 1;
        }
    }
    if start < body.len() {
        lines.push(&body[start..]);
    }
    lines
}

/// Byte spans of each line in `split_lines` order.
fn line_spans(body: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0;
    for line in split_lines(body) {
        spans.push((offset, offset + line.len()));
        offset += line.len();
    }
    spans
}

/// Splice replacements (in base space) into `body`, back to front so
/// offsets stay stable. pi's `applyReplacements`.
fn splice(body: &str, replacements: &[Matched]) -> String {
    let mut out = body.to_string();
    let mut ordered: Vec<&Matched> = replacements.iter().collect();
    ordered.sort_by_key(|m| m.index);
    for m in ordered.iter().rev() {
        out.replace_range(m.index..m.index + m.length, &m.new_text);
    }
    out
}

/// pi's `applyReplacementsPreservingUnchangedLines`: replacements that
/// matched in normalized space land line-wise, so lines no edit touches
/// keep their original bytes. The two views must hold the same line
/// count (normalization preserves newlines); a mismatch is an error,
///
/// never a misaligned write.
fn overlay(
    original: &str,
    base: &str,
    replacements: &[Matched],
    path: &str,
) -> Result<String, String> {
    let original_lines = split_lines(original);
    let base_spans = line_spans(base);
    if original_lines.len() != base_spans.len() {
        return Err(format!(
            "cannot apply the edit to {path}: normalization changed the line count"
        ));
    }
    // Group replacements by the lines they touch (pi's widening).
    let mut ordered: Vec<&Matched> = replacements.iter().collect();
    ordered.sort_by_key(|m| m.index);
    let mut groups: Vec<(usize, usize, Vec<&Matched>)> = Vec::new();
    for m in ordered {
        let start_line = base_spans
            .iter()
            .position(|&(s, e)| m.index >= s && m.index < e.max(s + 1))
            .ok_or_else(|| format!("the edit to {path} lands outside the file"))?;
        let mut end_line = start_line;
        while end_line < base_spans.len() && base_spans[end_line].1 < m.index + m.length {
            end_line += 1;
        }
        end_line += 1;
        let at = groups
            .len()
            .checked_sub(1)
            .filter(|&i| start_line < groups[i].1);
        match at {
            Some(i) => {
                groups[i].1 = groups[i].1.max(end_line);
                groups[i].2.push(m);
            }
            None => groups.push((start_line, end_line, vec![m])),
        }
    }
    let mut out = String::new();
    let mut line_at = 0;
    for (start_line, end_line, group) in &groups {
        for line in &original_lines[line_at..*start_line] {
            out.push_str(line);
        }
        let group_start = base_spans[*start_line].0;
        let group_end = base_spans[end_line - 1].1;
        let mut local: Vec<Matched> = group
            .iter()
            .map(|m| Matched {
                edit_index: m.edit_index,
                index: m.index - group_start,
                length: m.length,
                new_text: m.new_text.clone(),
            })
            .collect();
        local.sort_by_key(|m| m.index);
        out.push_str(&splice(&base[group_start..group_end], &local));
        line_at = *end_line;
    }
    for line in &original_lines[line_at..] {
        out.push_str(line);
    }
    Ok(out)
}

/// Match `edits` against LF `content` and return the replaced LF text:
/// pi's `applyEditsToNormalizedContent`, with this tool's error contract.
/// `new_text` values arrive LF-normalized by the caller.
pub fn apply_edits(
    content: &str,
    edits: &[(String, String)],
    path: &str,
) -> Result<String, String> {
    let (mut matched, any_fuzzy) = match_edits(content, edits, path)?;
    if !any_fuzzy {
        // Pure exact path: splice into the content as matched.
        matched.sort_by_key(|m| m.index);
        return Ok(splice(content, &matched));
    }
    // Any fuzzy hit moves the whole call into normalized space, then
    // overlays the touched lines back onto the original (pi's rule:
    // mixing spaces would misalign every offset after the first).
    let base = normalize_for_fuzzy_match(content);
    let mut normalized = Vec::with_capacity(matched.len());
    for m in &matched {
        let hit = fuzzy_find(&base, &edits[m.edit_index].0)
            .ok_or_else(|| format!("edits[{}].oldText not found in {path}", m.edit_index))?;
        normalized.push(Matched {
            edit_index: m.edit_index,
            index: hit.index,
            length: hit.length,
            new_text: m.new_text.clone(),
        });
    }
    normalized.sort_by_key(|m| m.index);
    for pair in normalized.windows(2) {
        if pair[0].index + pair[0].length > pair[1].index {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {path}; merge overlapping changes into one edit",
                pair[0].edit_index, pair[1].edit_index
            ));
        }
    }
    overlay(content, &base, &normalized, path)
}
