//! The edit tool's unified diff (EFG-014): display-only, computed from
//! the edit's own before/after strings - never by re-parsing anything
//! the transcript rendered. The shape is pi's `generateUnifiedPatch`:
//! `--- path`, `+++ path`, then `@@` hunks with context lines around
//! each change.

/// Context lines kept around each change (pi's `generateUnifiedPatch`
/// default).
pub const CONTEXT_LINES: usize = 4;

/// Beyond this many lines on a side of the trimmed middle, the change is
/// emitted as one replacement block instead of an LCS walk: an edit's
/// changes are localized, and a whole-file rewrite does not need a
/// minimal diff to be a correct one.
///
/// ponytail: O(n*m) DP over the trimmed middle (1000x1000 u32 = 4 MB);
/// a Myers walk with a linear footprint is the upgrade if huge rewrites
/// ever diff slowly.
const DP_LIMIT: usize = 1000;

/// One line of the edit script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op<'a> {
    /// Unchanged line.
    Equal(&'a str),
    /// Line only in the old text.
    Del(&'a str),
    /// Line only in the new text.
    Add(&'a str),
}

/// A unified diff of `old` → `new` for `path`, or an empty string when
/// the two are the same (a no-op claims nothing).
pub fn unified_diff(old: &str, new: &str, path: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Trim what the two already agree on: an edit touches a region, so
    // the common head and tail fall out before any comparing happens.
    let mut prefix = 0usize;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < old_lines.len() - prefix
        && suffix < new_lines.len() - prefix
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old_mid = &old_lines[prefix..old_lines.len() - suffix];
    let new_mid = &new_lines[prefix..new_lines.len() - suffix];
    if old_mid.is_empty() && new_mid.is_empty() {
        return String::new();
    }

    let middle = if old_mid.len() > DP_LIMIT || new_mid.len() > DP_LIMIT {
        let mut ops: Vec<Op<'_>> = old_mid.iter().map(|line| Op::Del(line)).collect();
        ops.extend(new_mid.iter().map(|line| Op::Add(line)));
        ops
    } else {
        lcs_script(old_mid, new_mid)
    };
    // The trimmed head and tail ride back in as context: the LCS runs
    // only over what the two disagree on, the diff shows what surrounds it.
    let mut ops: Vec<Op<'_>> = old_lines[..prefix]
        .iter()
        .map(|line| Op::Equal(line))
        .collect();
    ops.extend(middle);
    ops.extend(
        old_lines[old_lines.len() - suffix..]
            .iter()
            .map(|line| Op::Equal(line)),
    );
    render(&ops, path)
}

/// The edit script for the trimmed middle: longest common subsequence by
/// dynamic programming, backtracked into kept/removed/added lines.
fn lcs_script<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Op<'a>> {
    let width = old.len() + 1;
    let height = new.len() + 1;
    let mut table = vec![0u32; width * height];
    for i in 0..old.len() {
        for j in 0..new.len() {
            table[(j + 1) * width + (i + 1)] = if old[i] == new[j] {
                table[j * width + i] + 1
            } else {
                table[(j + 1) * width + i].max(table[j * width + (i + 1)])
            };
        }
    }
    let mut ops = Vec::with_capacity(old.len() + new.len());
    let (mut i, mut j) = (old.len(), new.len());
    while i > 0 && j > 0 {
        if old[i - 1] == new[j - 1] {
            ops.push(Op::Equal(old[i - 1]));
            i -= 1;
            j -= 1;
        } else if table[j * width + (i - 1)] > table[(j - 1) * width + i] {
            ops.push(Op::Del(old[i - 1]));
            i -= 1;
        } else {
            // Ties drain the new side first while walking backwards, so
            // the reversed script prints every removal before every
            // addition - the `-/ +` order unified diff expects.
            ops.push(Op::Add(new[j - 1]));
            j -= 1;
        }
    }
    while i > 0 {
        ops.push(Op::Del(old[i - 1]));
        i -= 1;
    }
    while j > 0 {
        ops.push(Op::Add(new[j - 1]));
        j -= 1;
    }
    ops.reverse();
    ops
}

/// Turn an edit script into unified-diff text: headers, then hunks that
/// keep `CONTEXT_LINES` around each change and merge when their contexts
/// touch.
fn render(ops: &[Op<'_>], path: &str) -> String {
    // Every row with the line numbers it occupies on each side (`None`
    // = the row is not on that side at all).
    #[derive(Clone, Copy)]
    struct Row<'a> {
        op: Op<'a>,
        old_no: Option<usize>,
        new_no: Option<usize>,
    }
    let mut rows: Vec<Row<'_>> = Vec::with_capacity(ops.len());
    let (mut old_no, mut new_no) = (1usize, 1usize);
    for op in ops {
        rows.push(Row {
            op: *op,
            old_no: match op {
                Op::Add(_) => None,
                _ => Some(old_no),
            },
            new_no: match op {
                Op::Del(_) => None,
                _ => Some(new_no),
            },
        });
        if !matches!(op, Op::Add(_)) {
            old_no += 1;
        }
        if !matches!(op, Op::Del(_)) {
            new_no += 1;
        }
    }

    // Window of each change, then merge windows whose gap the context
    // would already cover.
    let mut windows: Vec<(usize, usize)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if matches!(row.op, Op::Equal(_)) {
            continue;
        }
        let start = index.saturating_sub(CONTEXT_LINES);
        let end = (index + CONTEXT_LINES + 1).min(rows.len());
        match windows.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => windows.push((start, end)),
        }
    }
    if windows.is_empty() {
        return String::new();
    }

    let mut out = format!("--- {path}\n+++ {path}\n");
    for (start, end) in windows {
        let slice = &rows[start..end];
        // The side's line count, and where it starts: a side with no
        // rows in this hunk reports the insertion point (unified diff's
        // `-k,0` convention).
        let first_old = slice.iter().find_map(|row| row.old_no);
        let last_old = slice.iter().rev().find_map(|row| row.old_no);
        let first_new = slice.iter().find_map(|row| row.new_no);
        let last_new = slice.iter().rev().find_map(|row| row.new_no);
        let (old_start, old_count) = match (first_old, last_old) {
            (Some(first), Some(last)) => (first, last - first + 1),
            _ => (
                rows[..start]
                    .iter()
                    .filter(|row| row.old_no.is_some())
                    .count(),
                0,
            ),
        };
        let (new_start, new_count) = match (first_new, last_new) {
            (Some(first), Some(last)) => (first, last - first + 1),
            _ => (
                rows[..start]
                    .iter()
                    .filter(|row| row.new_no.is_some())
                    .count(),
                0,
            ),
        };
        out.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
        ));
        for row in slice {
            let (marker, line) = match row.op {
                Op::Equal(line) => (' ', line),
                Op::Del(line) => ('-', line),
                Op::Add(line) => ('+', line),
            };
            out.push(marker);
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // The shape: headers, one hunk, the three row kinds, and the line
    // numbers that let an editor jump to the change.
    #[test]
    fn one_change_is_one_hunk_with_context() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\ne\n";
        let diff = unified_diff(old, new, "f.txt");
        assert!(
            diff.starts_with("--- f.txt\n+++ f.txt\n@@ -1,5 +1,5 @@\n"),
            "{diff}"
        );
        let rows: Vec<&str> = diff.lines().skip(3).collect();
        assert_eq!(
            rows,
            vec![" a", "-b", "+B", " c", " d", " e"],
            "context around the change, removed then added: {diff}"
        );
    }

    // Two distant changes stay two hunks: context does not glue a whole
    // file together.
    #[test]
    fn distant_changes_stay_apart() {
        let old: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let mut new = old.clone();
        new = new
            .replace("line 0\n", "LINE 0\n")
            .replace("line 29\n", "LINE 29\n");
        let diff = unified_diff(&old, &new, "big.txt");
        assert_eq!(
            diff.lines().filter(|line| line.starts_with("@@ ")).count(),
            2,
            "{diff}"
        );
        assert!(
            !diff.contains(" line 15\n"),
            "the untouched middle is not shown: {diff}"
        );
    }

    // No change, no claim: the caller reads an empty string as "carry
    // nothing".
    #[test]
    fn an_identical_text_diffs_to_nothing() {
        assert_eq!(unified_diff("same\n", "same\n", "f.txt"), "");
        assert_eq!(unified_diff("", "", "f.txt"), "");
    }
}
