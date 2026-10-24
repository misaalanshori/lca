//! The attachment-token vocabulary: what starts a word, and which prose
//! wrapper is stripped.
//!
//! Split from `autocomplete.rs` (which is at the file-ceiling gate's edge)
//! and, more to the point, this is the **one home** of the delimiter set
//! gh #380 gap 1 asks for: a character is a word start, or it is not, and
//! every caller asks this module rather than spelling its own set.
//!
//! # The asymmetry this exists to preserve
//!
//! `@` and `/` are deliberately different, and conflating them is the
//! regression this cycle guards against:
//!
//! - `@` is a **reference marker**. It belongs wherever a word starts, so
//!   `Check @src/m`, `(@src/m` and `请查看，@src/m` all complete.
//! - `/` is a **command marker**, and only at line start. A mid-sentence `/`
//!   in `https://example.com/x` or `src/index.ts` must never open a command
//!   menu, so nothing here is used for the slash test: that check stays
//!   `text.starts_with('/')`.
//!
//! # Where the set comes from
//!
//! pi's `autocomplete.ts` splits the same question into three pieces, all of
//! which live in `packages/tui/src/`: `PATH_DELIMITERS` (space, tab, the two
//! quotes, `=`), `autocompleteSeparatorRegex` (whitespace **plus** CJK
//! punctuation, via `cjkPunctuationRegex`), and `stripLeadingWrappers` (the
//! `(()[]{}<>``)` family, stripped only while unclosed). This module is the
//! port of all three, kept together so the port has one address.

/// Prose wrappers that may precede a path, mapped to their closing
/// counterpart (pi's `PATH_WRAPPERS`).
///
/// Chosen so that `(@src/m` is prose-with-a-paren while `app/[slug]/pa` is a
/// route: the opener is stripped only while its closer is absent.
const WRAPPERS: [(char, char); 6] = [
    ('(', ')'),
    ('[', ']'),
    ('{', '}'),
    ('<', '>'),
    ('`', '`'),
    ('"', '"'),
];

/// The CJK punctuation ranges that separate prose from a token, as
/// `(first, last)` inclusive code-point pairs.
///
/// This is pi's `cjkPunctuationRegex` transcribed. That regex is
/// `(?:(?=\p{Punctuation})` + a CJK-script letter class + `)|[，．：；！？（）［］｛｝“”‘’…—])`,
/// i.e. "any punctuation adjacent to a CJK letter, or one of the listed
/// CJK-width forms"; evaluated over the whole code space it yields the 25
/// ranges below. Transcribed rather than computed because `lca-tui` may not
/// take a regex or Unicode-property dependency (ADR-0036), and a generated
/// table beats a hand-guessed one.
///
/// The tail entries (`U+FE45`, `U+16FE2`) are CJK-script punctuation marks
/// the property class catches; they are here so the transcription is
/// complete rather than approximately complete.
const CJK_PUNCTUATION: [(u32, u32); 25] = [
    (0x00B7, 0x00B7),   // middle dot
    (0x2014, 0x2014),   // em dash
    (0x2018, 0x2019),   // ‘ ’
    (0x201C, 0x201D),   // “ ”
    (0x2026, 0x2026),   // …
    (0x3001, 0x3003),   // 、。〃
    (0x3008, 0x3011),   // 〈〉《》「」『』【】
    (0x3014, 0x301F),   // 〔〕〖〗
    (0x3030, 0x3030),   // 〰
    (0x303D, 0x303D),   // 〽
    (0x30A0, 0x30A0),   // ゠
    (0x30FB, 0x30FB),   // ・
    (0xFE45, 0xFE46),   // ﹅﹆
    (0xFF01, 0xFF01),   // ！
    (0xFF08, 0xFF09),   // （）
    (0xFF0C, 0xFF0C),   // ，
    (0xFF0E, 0xFF0E),   // ．
    (0xFF1A, 0xFF1B),   // ：；
    (0xFF1F, 0xFF1F),   // ？
    (0xFF3B, 0xFF3B),   // ［
    (0xFF3D, 0xFF3D),   // ］
    (0xFF5B, 0xFF5B),   // ｛
    (0xFF5D, 0xFF5D),   // ｝
    (0xFF61, 0xFF65),   // halfwidth CJK punctuation
    (0x16FE2, 0x16FE2), // 𖿢
];

/// Whether `c` separates a phrase from the token that follows it.
///
/// Whitespace plus the CJK punctuation set. **Not** a path character: `/`,
/// `.`, `-`, `_` and `:` are all part of a word, which is what keeps
/// `src/index.ts` in one token.
#[must_use]
pub fn is_delimiter(c: char) -> bool {
    if c.is_whitespace() {
        return true;
    }
    let cp = c as u32;
    CJK_PUNCTUATION
        .iter()
        .any(|&(first, last)| (first..=last).contains(&cp))
}

/// Whether the text *before* a token ends at a word boundary.
///
/// `""` (start of line) counts. The final character decides; everything
/// before it is already inside the token.
#[must_use]
pub fn is_word_start(prefix: &str) -> bool {
    match prefix.chars().next_back() {
        None => true,
        Some(before) => is_delimiter(before),
    }
}

/// Strip prose wrappers from the front of `token` while each one is
/// **unclosed** (pi's `stripLeadingWrappers`).
///
/// `(@src/m` → `@src/m`, but `app/[slug]/pa` → unchanged, because the `[`
/// has its `]` later in the token and is therefore part of a route-shaped
/// path rather than prose.
#[must_use]
pub fn strip_wrappers(token: &str) -> &str {
    let mut rest = token;
    loop {
        let Some(first) = rest.chars().next() else {
            return rest;
        };
        let Some((_, closer)) = WRAPPERS.iter().find(|(open, _)| *open == first) else {
            return rest;
        };
        // Unclosed only: a wrapper whose closer appears later in the token is
        // part of the path.
        let body = &rest[first.len_utf8()..];
        if body.contains(*closer) {
            return rest;
        }
        rest = body;
    }
}

/// The attachment token at the end of `text`, if the cursor is in one.
///
/// Returns the query *after* the `@`, which is what the file walk wants.
/// `None` when the `@` is not at a word start (`a@b.com`, `查看@src`), which
/// is what keeps an email address from opening a file menu.
#[must_use]
pub fn attachment_query(text: &str) -> Option<&str> {
    // The token starts after the last delimiter, then loses any prose wrapper
    // in front of it.
    let start = text
        .char_indices()
        .rev()
        .find(|(_, c)| is_delimiter(*c))
        .map_or(0, |(index, c)| index + c.len_utf8());
    let token = strip_wrappers(&text[start..]);
    token.strip_prefix('@')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_and_cjk_punctuation_are_delimiters() {
        for c in [
            ' ', '\t', '\n', '\u{3000}', '，', '。', '：', '、', '「', '」', '…',
        ] {
            assert!(is_delimiter(c), "{c:?} separates a phrase");
        }
    }

    #[test]
    fn letters_and_path_characters_are_not_delimiters() {
        for c in [
            'a', 'Z', '0', '查', 'あ', '한', '々', 'Ａ', '/', '.', '-', '_', ':',
        ] {
            assert!(!is_delimiter(c), "{c:?} is part of a word");
        }
    }

    #[test]
    fn a_word_start_is_the_line_start_or_a_delimiter() {
        assert!(is_word_start(""));
        assert!(is_word_start("Check "));
        assert!(is_word_start("请查看，"));
        assert!(!is_word_start("user"));
        assert!(!is_word_start("查看"));
        assert!(!is_word_start("src/"));
    }

    #[test]
    fn only_an_unclosed_wrapper_is_stripped() {
        assert_eq!(strip_wrappers("(@src"), "@src");
        assert_eq!(strip_wrappers("([@src"), "@src");
        assert_eq!(strip_wrappers("app/[slug]/pa"), "app/[slug]/pa");
        assert_eq!(strip_wrappers("(marketing)/pa"), "(marketing)/pa");
        assert_eq!(strip_wrappers("@src"), "@src");
        assert_eq!(strip_wrappers(""), "");
    }

    #[test]
    fn the_query_is_what_follows_a_word_starting_at_sign() {
        assert_eq!(attachment_query("@src/m"), Some("src/m"));
        assert_eq!(attachment_query("Check @src/m"), Some("src/m"));
        assert_eq!(attachment_query("(@src/m"), Some("src/m"));
        assert_eq!(attachment_query("请查看，@src/m"), Some("src/m"));
        assert_eq!(attachment_query("a@b.com"), None);
        assert_eq!(attachment_query("查看@src"), None);
        assert_eq!(attachment_query("no marker"), None);
        assert_eq!(attachment_query("app/[slug]/pa"), None);
    }
}
