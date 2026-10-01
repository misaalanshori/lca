//! The math tokenizers markdown runs before rendering: pi's LaTeX
//! markdown extensions (`markdown.ts` §1, RE `markdown.md` §1) - block
//! `$$…$$` / `\[…\]` with the pending forms that keep streamed math raw,
//! and the inline `$…$` / `\(…\)` / `\[…\]` battery with its
//! false-positive guards.

/// A block-math match at line `i`.
pub(super) struct BlockMath {
    /// Lines consumed (the whole block, pending or closed).
    pub(super) consumed: usize,
    /// The math source between the delimiters.
    pub(super) content: String,
    /// Whether the block has not closed yet (streamed half-written math).
    pub(super) pending: bool,
}

/// pi's `looksLikePendingDollarMath`: a `$$` that never closes only
/// counts as pending math when the text *looks* like math, so prose that
/// mentions dollars stays prose.
pub(super) fn looks_like_pending_dollar_math(source: &str) -> bool {
    if source.contains('\\')
        && source
            .split('\\')
            .skip(1)
            .any(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
    {
        return true;
    }
    source.chars().any(|c| {
        matches!(
            c,
            '_' | '^'
                | '='
                | '+'
                | '*'
                | '/'
                | '<'
                | '>'
                | '('
                | ')'
                | '['
                | ']'
                | '|'
                | '±'
                | '≤'
                | '≥'
                | '≠'
                | '≈'
                | '∈'
                | '→'
                | '⇒'
                | '∞'
                | '∫'
                | '∑'
                | '√'
                | '-'
        )
    })
}

/// pi's `tokenizeBlockLatex`, in line space: a `$$`/`\[` opener at no
/// more than three spaces of indent, its closer at the end of some line,
/// or the pending form when nothing closes it.
pub(super) fn block_math(lines: &[&str], i: usize) -> Option<BlockMath> {
    let line = lines[i];
    if line.len() - line.trim_start().len() > 3 {
        return None;
    }
    let trimmed = line.trim_start();
    let (open, close): (&str, &str) = if trimmed.starts_with("$$") {
        ("$$", "$$")
    } else {
        ("\\[", "\\]")
    };
    let rest = trimmed.strip_prefix(open)?.trim_start_matches([' ', '\t']);

    // Same line: the closer must end the line (pi's regex), and the
    // content must be non-empty (`$$` `$$` is not a block).
    if let Some(content) = rest.trim_end_matches([' ', '\t']).strip_suffix(close)
        && !content.is_empty()
    {
        return Some(BlockMath {
            consumed: 1,
            content: content.to_string(),
            pending: false,
        });
    }

    // Multi-line: keep reading until a line ends with the closer.
    let mut content_lines: Vec<&str> = vec![rest];
    let mut j = i + 1;
    while j < lines.len() {
        let candidate = lines[j].trim_end_matches([' ', '\t']);
        if let Some(stripped) = candidate.strip_suffix(close) {
            content_lines.push(stripped);
            let content = content_lines.join("\n");
            return Some(BlockMath {
                consumed: j - i + 1,
                content,
                pending: false,
            });
        }
        content_lines.push(lines[j]);
        j += 1;
    }

    // Pending (streamed, still open): pi keeps it raw when it looks like
    // math, and renders plain text otherwise.
    let content = content_lines.join("\n");
    let looks_math = open == "\\[" || looks_like_pending_dollar_math(&content);
    if !looks_math {
        return None;
    }
    Some(BlockMath {
        consumed: lines.len() - i,
        content,
        pending: true,
    })
}

/// An inline-math match starting at `i`.
pub(super) struct InlineMath {
    /// The math source between the delimiters.
    pub(super) content: String,
    /// Whether the closer never arrived (streamed half-written math).
    pub(super) pending: bool,
    /// Characters consumed from `i`.
    pub(super) len: usize,
}

fn is_escaped(chars: &[char], index: usize) -> bool {
    let mut backslashes = 0;
    let mut j = index;
    while j > 0 && chars[j - 1] == '\\' {
        backslashes += 1;
        j -= 1;
    }
    backslashes % 2 == 1
}

/// Escape-aware search for `closing` starting at `start` (pi's
/// `findClosingDelimiter`).
fn find_closing(chars: &[char], closing: &[char], start: usize) -> Option<usize> {
    let mut at = start;
    while at + closing.len() <= chars.len() {
        if chars[at..at + closing.len()] == *closing && !is_escaped(chars, at) {
            return Some(at);
        }
        at += 1;
    }
    None
}

/// pi's `tokenizeInlineLatex`: `$…$` (not `$ `), `\(…\)`, `\[…\]`, with
/// the dollar-prose false-positive guards and the pending forms.
pub(super) fn inline_math(chars: &[char], i: usize) -> Option<InlineMath> {
    let (open, close): (&[char], &[char]) = if chars[i] == '$' {
        if chars.get(i + 1).is_some_and(|c| c.is_whitespace()) {
            return None;
        }
        if chars.get(i + 1) == Some(&'$') {
            (&['$', '$'], &['$', '$'])
        } else {
            (&['$'], &['$'])
        }
    } else if chars.get(i + 1) == Some(&'(') {
        (&['\\', '('], &['\\', ')'])
    } else if chars.get(i + 1) == Some(&'[') {
        (&['\\', '['], &['\\', ']'])
    } else {
        return None;
    };

    let body_start = i + open.len();
    match find_closing(chars, close, body_start) {
        Some(close_at) => {
            let content: String = chars[body_start..close_at].iter().collect();
            if open == ['$', '$'] && content.is_empty() {
                return None;
            }
            if open == ['$'] {
                // pi's dollar-prose guards: a trailing space inside, a
                // digit right after, an UPPER identifier that is followed
                // by more identifier (`$FOO bar`), or a backtick inside.
                let after = chars
                    .get(close_at + close.len())
                    .copied()
                    .unwrap_or('\u{0}');
                let last = content.chars().last().unwrap_or(' ');
                let letters: Vec<char> = content.chars().collect();
                let mut is_upper_ident =
                    !letters.is_empty() && (letters[0].is_ascii_uppercase() || letters[0] == '_');
                if is_upper_ident {
                    for (idx, c) in letters.iter().enumerate().skip(1) {
                        if c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_' {
                            continue;
                        }
                        let last_char = idx + 1 == letters.len();
                        if last_char && !c.is_whitespace() && !c.is_alphanumeric() {
                            continue;
                        }
                        is_upper_ident = false;
                        break;
                    }
                }
                let after_is_ident = after.is_ascii_alphanumeric() || after == '_';
                if last.is_whitespace()
                    || after.is_ascii_digit()
                    || (is_upper_ident && after_is_ident)
                    || content.contains('`')
                {
                    return None;
                }
            }
            let len = close_at + close.len() - i;
            Some(InlineMath {
                content,
                pending: false,
                len,
            })
        }
        None => {
            // Pending only for `\(`/`\[` or a dollar run that looks like
            // math (pi's rule) - and never for a bare `$ `.
            let rest: String = chars[body_start..].iter().collect();
            let looks_math = if open != ['$'] {
                true
            } else {
                looks_like_pending_dollar_math(&rest)
            };
            if !looks_math {
                return None;
            }
            Some(InlineMath {
                content: rest,
                pending: true,
                len: chars.len() - i,
            })
        }
    }
}
