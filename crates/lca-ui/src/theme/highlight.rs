//! Syntax highlighting for fenced code blocks: pi's `highlightCode`
//! (`theme.md` §6, `theme.ts` `getCliHighlightTheme` + `highlightCode`).
//!
//! pi shells out to cli-highlight (highlight.js) with a **token-mapped
//! theme** - ~20 scopes painted from the `syntax*` roles - and deliberately
//! disables language auto-detection, because "cli-highlight's auto-detection
//! is unreliable and can misidentify prose as AppleScript, LiveCodeServer,
//! etc., coloring random English words as keywords". This port keeps both
//! rules: only a fence that *names* a language we know gets highlighted,
//! and everything else falls back to pi's unknown-language path (the whole
//! block in `mdCodeBlock`).
//!
//! ponytail: the grammars here are hand-written token classes for the
//! languages a coding transcript actually shows, not a highlight.js port -
//! the roles and the scope names are pi's, the parsing is one scanner. A
//! grammar gap (a language pi knows and this does not) falls back to
//! `mdCodeBlock` exactly as pi does for an unknown language; upgrade to
//! `syntect` if a real miss costs more than the binary-size budget.

use crate::theme::StyleFn;

/// The nine `syntax*` roles, resolved once per render (pi's
/// `getCliHighlightTheme` cache). `None` on an operator/punctuation role
/// means "paints exactly like plain text in this palette" - pi's own
/// defaults are `#d4d4d4`, the text color - so the block stays free of
/// no-op escape churn.
#[derive(Clone)]
pub struct SyntaxStyles {
    /// `syntaxComment`.
    pub comment: StyleFn,
    /// `syntaxKeyword`.
    pub keyword: StyleFn,
    /// `syntaxFunction`.
    pub function: StyleFn,
    /// `syntaxVariable`.
    pub variable: StyleFn,
    /// `syntaxString`.
    pub string: StyleFn,
    /// `syntaxNumber`.
    pub number: StyleFn,
    /// `syntaxType`.
    pub type_: StyleFn,
    /// `syntaxOperator`, when it differs from the text color.
    pub operator: Option<StyleFn>,
    /// `syntaxPunctuation`, when it differs from the text color.
    pub punctuation: Option<StyleFn>,
}

/// What one language needs from the scanner.
#[derive(Clone, Copy)]
struct Lang {
    /// `//`-style comment introducers.
    line_comment: &'static [&'static str],
    /// A block comment's open/close pair, when the language has one.
    block_comment: Option<(&'static str, &'static str)>,
    /// The characters that open a string literal.
    strings: &'static str,
    /// Reserved words.
    keywords: &'static [&'static str],
    /// Type-like names (highlight.js's `built_in`/`type`/`class`).
    types: &'static [&'static str],
}

const JS_STRINGS: &str = "'\"`";
const C_STRINGS: &str = "'\"";

const JAVASCRIPT: Lang = Lang {
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    strings: JS_STRINGS,
    keywords: &[
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "export",
        "extends",
        "finally",
        "for",
        "from",
        "function",
        "get",
        "if",
        "import",
        "in",
        "instanceof",
        "let",
        "new",
        "of",
        "return",
        "set",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "try",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ],
    types: &[
        "Array",
        "Boolean",
        "Date",
        "Error",
        "JSON",
        "Map",
        "Math",
        "Number",
        "Object",
        "Promise",
        "RegExp",
        "Set",
        "String",
        "Symbol",
        "console",
        "document",
        "false",
        "null",
        "true",
        "undefined",
        "window",
    ],
};

const TYPESCRIPT: Lang = Lang {
    line_comment: JAVASCRIPT.line_comment,
    block_comment: JAVASCRIPT.block_comment,
    strings: JAVASCRIPT.strings,
    keywords: &[
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "declare",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "finally",
        "for",
        "from",
        "function",
        "get",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "keyof",
        "let",
        "namespace",
        "new",
        "of",
        "private",
        "protected",
        "public",
        "readonly",
        "return",
        "set",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "try",
        "type",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ],
    types: &[
        "any",
        "Array",
        "boolean",
        "Boolean",
        "never",
        "null",
        "number",
        "Number",
        "object",
        "Promise",
        "Record",
        "string",
        "String",
        "symbol",
        "true",
        "undefined",
        "unknown",
    ],
};

const PYTHON: Lang = Lang {
    line_comment: &["#"],
    block_comment: None,
    strings: "'\"",
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "case", "class", "continue", "def",
        "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in",
        "is", "lambda", "match", "nonlocal", "not", "or", "pass", "raise", "return", "try",
        "while", "with", "yield",
    ],
    types: &[
        "bool",
        "bytes",
        "dict",
        "Exception",
        "float",
        "frozenset",
        "int",
        "list",
        "None",
        "object",
        "self",
        "set",
        "str",
        "tuple",
        "True",
        "False",
        "type",
    ],
};

const GO: Lang = Lang {
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    strings: "\"`",
    keywords: &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "interface",
        "map",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "type",
        "var",
    ],
    types: &[
        "any",
        "bool",
        "byte",
        "complex128",
        "complex64",
        "error",
        "float32",
        "float64",
        "int",
        "int16",
        "int32",
        "int64",
        "int8",
        "nil",
        "rune",
        "string",
        "true",
        "false",
        "uint",
        "uint16",
        "uint32",
        "uint64",
        "uint8",
        "uintptr",
    ],
};

const C: Lang = Lang {
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    strings: C_STRINGS,
    keywords: &[
        "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern",
        "for", "goto", "if", "inline", "register", "restrict", "return", "sizeof", "static",
        "struct", "switch", "typedef", "union", "volatile", "while",
    ],
    types: &[
        "bool", "char", "double", "FILE", "float", "int", "long", "short", "signed", "size_t",
        "ssize_t", "uint16_t", "uint32_t", "uint64_t", "uint8_t", "int16_t", "int32_t", "int64_t",
        "int8_t", "unsigned", "void", "va_list",
    ],
};

const CPP: Lang = Lang {
    line_comment: C.line_comment,
    block_comment: C.block_comment,
    strings: C.strings,
    keywords: &[
        "alignas",
        "auto",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "constexpr",
        "continue",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "explicit",
        "export",
        "extern",
        "for",
        "friend",
        "goto",
        "if",
        "inline",
        "mutable",
        "namespace",
        "new",
        "noexcept",
        "nullptr",
        "operator",
        "override",
        "private",
        "protected",
        "public",
        "return",
        "sizeof",
        "static",
        "struct",
        "switch",
        "template",
        "this",
        "throw",
        "try",
        "typedef",
        "typename",
        "union",
        "using",
        "virtual",
        "while",
    ],
    types: &[
        "bool",
        "char",
        "double",
        "float",
        "int",
        "long",
        "short",
        "signed",
        "std",
        "string",
        "unsigned",
        "void",
        "size_t",
        "uint32_t",
        "int32_t",
        "vector",
        "map",
        "unordered_map",
    ],
};

const JAVA: Lang = Lang {
    line_comment: &["//"],
    block_comment: Some(("/*", "*/")),
    strings: C_STRINGS,
    keywords: &[
        "abstract",
        "assert",
        "break",
        "case",
        "catch",
        "class",
        "continue",
        "default",
        "do",
        "else",
        "enum",
        "extends",
        "final",
        "finally",
        "for",
        "if",
        "implements",
        "import",
        "instanceof",
        "interface",
        "new",
        "package",
        "private",
        "protected",
        "public",
        "record",
        "return",
        "sealed",
        "static",
        "super",
        "switch",
        "synchronized",
        "this",
        "throw",
        "throws",
        "try",
        "var",
        "while",
        "yield",
    ],
    types: &[
        "boolean", "byte", "char", "double", "false", "float", "int", "Integer", "List", "Long",
        "Map", "null", "Object", "String", "true", "void",
    ],
};

const SHELL: Lang = Lang {
    line_comment: &["#"],
    block_comment: None,
    strings: "'\"",
    keywords: &[
        "alias", "bg", "break", "case", "cd", "command", "continue", "declare", "do", "done",
        "elif", "else", "esac", "eval", "exec", "exit", "export", "fi", "fg", "for", "function",
        "if", "in", "local", "printf", "read", "readonly", "return", "select", "set", "shift",
        "source", "then", "time", "trap", "true", "false", "unset", "until", "wait", "while",
    ],
    types: &[
        "echo", "export", "sudo", "apt", "cargo", "git", "npm", "python", "sh", "bash",
    ],
};

const SQL: Lang = Lang {
    line_comment: &["--"],
    block_comment: Some(("/*", "*/")),
    strings: "'\"",
    keywords: &[
        "alter", "and", "as", "asc", "between", "by", "create", "delete", "desc", "drop", "exists",
        "from", "group", "having", "in", "index", "insert", "into", "is", "join", "key", "left",
        "limit", "not", "null", "on", "or", "order", "outer", "primary", "right", "select", "set",
        "table", "union", "unique", "update", "values", "where",
    ],
    types: &[
        "bigint",
        "boolean",
        "char",
        "date",
        "datetime",
        "decimal",
        "double",
        "float",
        "int",
        "json",
        "serial",
        "text",
        "timestamp",
        "varchar",
    ],
};

const JSON: Lang = Lang {
    line_comment: &[],
    block_comment: None,
    strings: "\"",
    keywords: &[],
    types: &["false", "true", "null"],
};

const YAML: Lang = Lang {
    line_comment: &["#"],
    block_comment: None,
    strings: "'\"",
    keywords: &[],
    types: &["true", "false", "null", "yes", "no", "on", "off"],
};

const TOML: Lang = Lang {
    line_comment: &["#"],
    block_comment: None,
    strings: "'\"",
    keywords: &["true", "false"],
    types: &[],
};

/// Resolve a fence's language tag (pi's `supportsLanguage` gate, minus the
/// auto-detection pi disabled on purpose).
fn language(lang: &str) -> Option<Lang> {
    let tag = lang.trim().to_ascii_lowercase();
    Some(match tag.as_str() {
        "rust" | "rs" => Lang {
            line_comment: &["//"],
            block_comment: Some(("/*", "*/")),
            strings: "\"'",
            keywords: &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
                "enum", "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
                "move", "mut", "pub", "ref", "return", "self", "static", "struct", "super",
                "trait", "type", "unsafe", "use", "where", "while",
            ],
            types: &[
                "Self", "String", "Vec", "Option", "Result", "Box", "Rc", "Arc", "bool", "char",
                "f32", "f64", "i128", "i16", "i32", "i64", "i8", "isize", "str", "u128", "u16",
                "u32", "u64", "u8", "usize",
            ],
        },
        "javascript" | "js" | "jsx" | "mjs" | "cjs" => JAVASCRIPT,
        "typescript" | "ts" | "tsx" => TYPESCRIPT,
        "python" | "py" | "python3" => PYTHON,
        "go" | "golang" => GO,
        "c" | "h" => C,
        "cpp" | "c++" | "cc" | "hpp" | "cxx" => CPP,
        "java" => JAVA,
        "sh" | "bash" | "zsh" | "shell" | "console" | "shell-session" => SHELL,
        "sql" => SQL,
        "json" | "jsonc" | "json5" => JSON,
        "yaml" | "yml" => YAML,
        "toml" => TOML,
        _ => return None,
    })
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Highlight `code` for a known `lang`; `None` when the tag names a
/// language this port does not know (the caller paints `mdCodeBlock`, the
/// same fallback pi takes for an unknown language).
pub fn highlight(code: &str, lang: &str, styles: &SyntaxStyles) -> Option<Vec<String>> {
    let spec = language(lang)?;
    let chars: Vec<char> = code.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut i = 0usize;
    // Resolved once: the scanner consults these at every position.
    let line_comments: Vec<Vec<char>> = spec
        .line_comment
        .iter()
        .map(|intro| intro.chars().collect())
        .collect();
    let block: Option<(Vec<char>, Vec<char>)> = spec
        .block_comment
        .map(|(open, close)| (open.chars().collect(), close.chars().collect()));

    let push = |line: &mut String, text: &str, style: &StyleFn| line.push_str(&style(text));

    while i < chars.len() {
        let c = chars[i];

        // Newline: flush the line.
        if c == '\n' {
            out.push(std::mem::take(&mut line));
            i += 1;
            continue;
        }

        // Comments (block comments run across lines).
        let mut matched = false;
        for intro in &line_comments {
            if !intro.is_empty() && chars[i..].starts_with(&intro[..]) {
                let end = chars[i..]
                    .iter()
                    .position(|ch| *ch == '\n')
                    .map(|p| i + p)
                    .unwrap_or(chars.len());
                let text: String = chars[i..end].iter().collect();
                push(&mut line, &text, &styles.comment);
                i = end;
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        if let Some((open, close)) = &block
            && chars[i..].starts_with(&open[..])
        {
            let mut end = chars.len();
            let mut j = i + open.len();
            while j + close.len() <= chars.len() {
                if chars[j..].starts_with(&close[..]) {
                    end = j + close.len();
                    break;
                }
                j += 1;
            }
            // A comment that never closes runs to the end of the
            // block, which is what a compiler does too.
            let text: String = chars[i..end].iter().collect();
            for (n, part) in text.split('\n').enumerate() {
                if n > 0 {
                    out.push(std::mem::take(&mut line));
                }
                push(&mut line, part, &styles.comment);
            }
            i = end;
            continue;
        }

        // Strings (with escapes).
        if spec.strings.contains(c) {
            let quote = c;
            let mut j = i + 1;
            while j < chars.len() {
                if chars[j] == '\\' && j + 1 < chars.len() {
                    j += 2;
                    continue;
                }
                if chars[j] == quote {
                    j += 1;
                    break;
                }
                if chars[j] == '\n' && quote != '`' {
                    // An unterminated single-line string ends at the line.
                    j += 1;
                    break;
                }
                j += 1;
            }
            let text: String = chars[i..j].iter().collect();
            for (n, part) in text.split('\n').enumerate() {
                if n > 0 {
                    out.push(std::mem::take(&mut line));
                }
                push(&mut line, part, &styles.string);
            }
            i = j;
            continue;
        }

        // Numbers.
        if c.is_ascii_digit() {
            let mut j = i;
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric() || chars[j] == '.' || chars[j] == '_')
            {
                j += 1;
            }
            let text: String = chars[i..j].iter().collect();
            push(&mut line, &text, &styles.number);
            i = j;
            continue;
        }

        // Identifiers: keyword, type, a call, or a plain variable.
        if is_ident_start(c) {
            let mut j = i;
            while j < chars.len() && is_ident(chars[j]) {
                j += 1;
            }
            let text: String = chars[i..j].iter().collect();
            let rest: String = chars[j..].iter().take_while(|ch| !is_ident(**ch)).collect();
            let called = rest.trim_start().starts_with('(');
            let style = if spec.keywords.contains(&text.as_str()) {
                &styles.keyword
            } else if spec.types.contains(&text.as_str()) {
                &styles.type_
            } else if called {
                &styles.function
            } else {
                &styles.variable
            };
            push(&mut line, &text, style);
            i = j;
            continue;
        }

        // Operators and punctuation, only when the palette paints them
        // differently from plain text (pi's defaults do not).
        if "+-*/%=<>!&|^~?:.".contains(c) {
            if let Some(style) = &styles.operator {
                push(&mut line, &c.to_string(), style);
            } else {
                line.push(c);
            }
            i += 1;
            continue;
        }
        if "(){}[],;#@$".contains(c) {
            if let Some(style) = &styles.punctuation {
                push(&mut line, &c.to_string(), style);
            } else {
                line.push(c);
            }
            i += 1;
            continue;
        }

        line.push(c);
        i += 1;
    }
    out.push(line);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A styles table that tags each class, so a test can read the classes
    /// off the output instead of matching real palette bytes.
    fn tagged() -> SyntaxStyles {
        let t = |name: &'static str| -> StyleFn {
            Arc::new(move |text| format!("<{name}>{text}</{name}>"))
        };
        SyntaxStyles {
            comment: t("comment"),
            keyword: t("keyword"),
            function: t("function"),
            variable: t("variable"),
            string: t("string"),
            number: t("number"),
            type_: t("type"),
            operator: None,
            punctuation: None,
        }
    }

    fn plain(lines: &[String]) -> String {
        lines.join("\n")
    }

    // Verifies: R3 - an unknown fence takes pi's fallback path: `None`, so
    // the caller paints the whole block `mdCodeBlock` (and no word in prose
    // ever becomes a keyword - pi's reason for killing auto-detection).
    #[test]
    fn an_unknown_language_is_not_highlighted() {
        assert!(highlight("x = 1", "applescript-ish", &tagged()).is_none());
        assert!(highlight("plain prose", "", &tagged()).is_none());
    }

    // Verifies: R3 - each documented class lands on its own token.
    #[test]
    fn rust_tokens_take_their_classes() {
        let out = highlight(
            "// note\nfn greet(name: &str) -> String {\n    let n = 42;\n    format!(\"hi {name}\")\n}",
            "rust",
            &tagged(),
        )
        .expect("rust is known");
        let text = plain(&out);
        assert!(text.contains("<comment>// note</comment>"), "{text}");
        assert!(text.contains("<keyword>fn</keyword>"), "{text}");
        assert!(text.contains("<function>greet</function>("), "{text}");
        assert!(text.contains("<type>String</type>"), "{text}");
        assert!(text.contains("<keyword>let</keyword>"), "{text}");
        assert!(text.contains("<number>42</number>"), "{text}");
        assert!(text.contains("<string>\"hi {name}\"</string>"), "{text}");
        assert!(text.contains("<variable>n</variable>"), "{text}");
    }

    // Verifies: R3 - aliases resolve and a language without block comments
    // still stops its line comment at the newline.
    #[test]
    fn aliases_and_line_comments_resolve() {
        let py = highlight("def f():\n    return True  # ok", "py", &tagged()).expect("python");
        let text = plain(&py);
        assert!(text.contains("<keyword>def</keyword>"), "{text}");
        assert!(text.contains("<comment># ok</comment>"), "{text}");
        assert_eq!(py.len(), 2, "two lines out: {py:?}");
        let sh = highlight("echo hi # note", "bash", &tagged()).expect("shell");
        assert!(plain(&sh).contains("<comment># note</comment>"));
    }

    // Verifies: R3 - a block comment spanning lines keeps its class and
    // does not swallow the code after it closes.
    #[test]
    fn a_block_comment_spans_lines_then_code_resumes() {
        let out = highlight("/* one\n   two */\nint x = 1;", "c", &tagged()).expect("c is known");
        assert_eq!(out.len(), 3, "{out:?}");
        assert!(out[0].contains("<comment>/* one"), "{out:?}");
        assert!(out[1].contains("two */</comment>"), "{out:?}");
        assert!(out[2].contains("<type>int</type>"), "{out:?}");
    }

    // Verifies: R3 - a string containing a comment introducer or a keyword
    // stays a string (the scanner's order matters).
    #[test]
    fn comment_markers_inside_strings_stay_strings() {
        let out = highlight(r#"let s = "// not a comment";"#, "rust", &tagged()).expect("rust");
        let text = plain(&out);
        assert!(
            text.contains("<string>\"// not a comment\"</string>"),
            "{text}"
        );
        assert!(!text.contains("<comment>"), "{text}");
    }
}
