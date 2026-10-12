//! Skill frontmatter parsing (gh #156): the single shared parser
//! behind the host merge (`lca-tools`) and the `skills` extension.
//! Guest-safe (no I/O, no dependencies beyond `alloc` patterns), so
//! the WASM guest links it exactly like the host does.

/// One parsed `SKILL.md`: every field either implementation reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSkill {
    /// The `name:` header, else the fallback (directory or stem).
    pub name: String,
    /// The `description:` header (the host advertises it; guests that
    /// never read it ignore it).
    pub description: String,
    /// Lowercase trigger words from `match:`.
    pub match_words: Vec<String>,
    /// False when `disable-model-invocation: true`.
    pub model_invocable: bool,
    /// The instruction body after the `---` line.
    pub body: String,
}

/// Parse one `SKILL.md` (the standard Claude format): leading
/// `key: value` header lines, then `---`, then the body. An empty
/// line or a non-header line ends the headers; unknown header keys
/// are reserved, not errors.
pub fn parse_skill_text(fallback_name: &str, text: &str) -> ParsedSkill {
    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut match_line = String::new();
    let mut model_invocable = true;
    let mut body_start = 0usize;
    let mut consumed = 0usize;
    for line in text.split('\n') {
        let trimmed = line.trim();
        body_start = consumed;
        consumed += line.len() + 1;
        if trimmed.is_empty() || trimmed == "---" {
            body_start = consumed;
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            match key.trim() {
                "name" => name = value.trim().to_string(),
                "description" => description = value.trim().to_string(),
                "match" => match_line = value.to_string(),
                "disable-model-invocation" => {
                    model_invocable = value.trim() != "true";
                }
                _ => {} // unknown header keys are reserved, not errors
            }
        } else {
            body_start = consumed - line.len() - 1;
            break;
        }
    }
    let body = text.get(body_start..).unwrap_or("").trim().to_string();
    let match_words = match_line
        .split(',')
        .map(|word| word.trim().to_lowercase())
        .filter(|word| !word.is_empty())
        .collect();
    ParsedSkill {
        name,
        description,
        match_words,
        model_invocable,
        body,
    }
}
