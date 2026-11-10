//! The lexical layer: cut a command into segments, then a segment into
//! quote-aware tokens. No path context and no policy live here.
//!
//! Split from the analyzer file for gate 11; the scanner, its comments and its
//! guards are unchanged (its rows live on in `tests.rs`, under the same
//! `shell::tests::<row>` paths).

// ---------------------------------------------------------------------------
// A minimal, quote-aware shell scanner
// ---------------------------------------------------------------------------

/// Split on unquoted `;`, `&`, `|`, and newlines.
pub(super) fn segments(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut state = Quote::None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match state {
            Quote::None => match c {
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push('\\');
                        current.push(next);
                    }
                }
                '\'' => {
                    state = Quote::Single;
                    current.push(c);
                }
                '"' => {
                    state = Quote::Double;
                    current.push(c);
                }
                ';' | '&' | '|' | '\n' => {
                    let segment = std::mem::take(&mut current);
                    if !segment.trim().is_empty() {
                        out.push(segment);
                    }
                }
                _ => current.push(c),
            },
            Quote::Single => {
                current.push(c);
                if c == '\'' {
                    state = Quote::None;
                }
            }
            Quote::Double => {
                current.push(c);
                if c == '"' {
                    state = Quote::None;
                } else if c == '\\'
                    && let Some(next) = chars.next()
                {
                    current.push(next);
                }
            }
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

#[derive(Clone, Copy)]
enum Quote {
    None,
    Single,
    Double,
}

/// Split one segment into tokens: unquoted whitespace separates; quotes group
/// and are stripped; redirection operators stay attached to their target when
/// written without a space.
pub(super) fn tokens(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut state = Quote::None;
    let mut chars = segment.chars().peekable();
    let flush = |current: &mut String, out: &mut Vec<String>| {
        if !current.is_empty() {
            out.push(std::mem::take(current));
        }
    };
    while let Some(c) = chars.next() {
        match state {
            Quote::None => match c {
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                '\'' => state = Quote::Single,
                '"' => state = Quote::Double,
                c if c.is_whitespace() => flush(&mut current, &mut out),
                _ => current.push(c),
            },
            Quote::Single => {
                if c == '\'' {
                    state = Quote::None;
                } else {
                    current.push(c);
                }
            }
            Quote::Double => {
                if c == '"' {
                    state = Quote::None;
                } else if c == '\\'
                    && let Some(next) = chars.next()
                {
                    current.push(next);
                } else {
                    current.push(c);
                }
            }
        }
    }
    flush(&mut current, &mut out);
    out
}
