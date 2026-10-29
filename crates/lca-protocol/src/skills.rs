//! Skill frontmatter parsing (gh #156, gh #304): the single shared parser
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
    /// The instruction body after the header.
    pub body: String,
}

/// The header fields this parser understands.
///
/// This list **is** the distinction gh #304 turns on: a line whose key is one of
/// these is header chrome, and a line whose key is not is the first line of the
/// body. Matched exactly, as YAML keys are.
const HEADER_KEYS: [&str; 4] = ["name", "description", "match", "disable-model-invocation"];

/// The fence that opens and closes the bounded header dialect.
const FENCE: &str = "---";

/// The four fields, accumulated as lines are read.
struct Fields {
    name: String,
    description: String,
    match_line: String,
    model_invocable: bool,
}

impl Fields {
    fn new(fallback_name: &str) -> Fields {
        Fields {
            name: fallback_name.to_string(),
            description: String::new(),
            match_line: String::new(),
            model_invocable: true,
        }
    }

    /// Apply one header field. The caller has already established that `key` is in
    /// [`HEADER_KEYS`], so this cannot silently ignore anything.
    fn apply(&mut self, key: &str, value: &str) {
        match key {
            "name" => self.name = value.trim().to_string(),
            "description" => self.description = value.trim().to_string(),
            "match" => self.match_line = value.to_string(),
            // Only the literal `true` restricts. Anything else - `false`, or a typo -
            // leaves the skill invocable, which is the permissive reading, and
            // matching pi's `frontmatter["disable-model-invocation"] === true`.
            _ => self.model_invocable = value.trim() != "true",
        }
    }
}

/// `(key, value)` when `line` is a header field line, else `None`.
///
/// The split is the **first** colon, so a value may contain colons
/// (`description: Use when: X happened` keeps its whole tail as the value).
///
/// No whitespace rule is needed for the key: the comparison is *exact* against
/// [`HEADER_KEYS`], and no member of that list contains a space, so `this is: prose`
/// and `Note: this repo needs Rust 1.85.` fail the lookup on the key and are prose by
/// construction. An explicit "a key may not contain whitespace" check would be a
/// second rule restating what the list already says - and it would be unreachable,
/// which is how a rule rots.
fn header_field(line: &str) -> Option<(&'static str, &str)> {
    let (key, value) = line.split_once(':')?;
    let key = key.trim();
    HEADER_KEYS
        .iter()
        .find(|known| **known == key)
        .map(|known| (*known, value))
}

/// Whether `line` is the fence, exactly. A line of dashes with anything else on it
/// is not a fence, so `--- see below` stays whatever the surrounding dialect makes
/// of it rather than silently becoming chrome.
fn is_fence(line: &str) -> bool {
    line.trim() == FENCE
}

/// The byte offset just past the opening fence, when the file opens with one.
///
/// Leading blank lines are tolerated - a file that starts with a newline still has
/// its fence first in any meaningful sense - but nothing else is: a field line
/// before the fence means the file is the headerless dialect.
fn opening_fence_end(text: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in text.split('\n') {
        if line.trim().is_empty() {
            offset += line.len() + 1;
            continue;
        }
        return is_fence(line).then_some(offset + line.len() + 1);
    }
    None
}

/// The bounded dialect: everything between the fences is a header region.
///
/// Because the author fenced the region off and called it a header, **every** line
/// in it is chrome - an unknown key here is reserved rather than prose, which is
/// the one place that reading is safe.
///
/// **Two passes, because an unterminated fence means the file is not a header at
/// all** - the whole text is body. Applying fields while scanning would leave a
/// half-parsed header behind in that case: the file would come back as body *and*
/// with `name:` already set, which is an interpretation the author never wrote.
/// So the closing fence is found first, and the region is read only once it is
/// known to be a region.
fn parse_bounded(text: &str, after_open: usize, fields: &mut Fields) -> usize {
    let region = text.get(after_open..).unwrap_or("");
    let mut scan = after_open;
    let mut close: Option<usize> = None;
    for line in region.split('\n') {
        let line_end = scan + line.len() + 1;
        if is_fence(line) {
            close = Some(line_end);
            break;
        }
        scan = line_end;
    }
    let Some(body_start) = close else {
        // No closing fence: not a header. Nothing was applied.
        return 0;
    };
    let fenced = text
        .get(after_open..body_start.saturating_sub(1))
        .unwrap_or("");
    for line in fenced.split('\n') {
        if is_fence(line) {
            continue;
        }
        if let Some((key, value)) = header_field(line.trim()) {
            fields.apply(key, value);
        }
        // An unknown line inside the fence is reserved chrome (gh #304): the
        // author declared this region a header, so it is not prose.
    }
    body_start
}

/// The headerless dialect: parse the leading run of field lines, and stop at the
/// first line that is not one.
///
/// Three things end the header, and all three are body starts:
///
/// * a **blank** line (as gh #156 always had it);
/// * a **fence**, which is the trailing-`---` form the repo's own fixtures use
///   (`name: x\n---\nbody`);
/// * a line whose key is **not** one of [`HEADER_KEYS`] - gh #304's fix. It used to
///   be consumed as reserved chrome and its text lost; now it is the body's first
///   line.
fn parse_headerless(text: &str, fields: &mut Fields) -> usize {
    let mut offset = 0usize;
    for line in text.split('\n') {
        // Before the field check, because both are terminators rather than lines.
        if line.trim().is_empty() || is_fence(line) {
            return offset + line.len() + 1;
        }
        let Some((key, value)) = header_field(line.trim()) else {
            // Gh #304: not a field, so this is where the body begins - the line
            // itself is not consumed.
            return offset;
        };
        fields.apply(key, value);
        offset += line.len() + 1;
    }
    // Every line was a field: no body.
    text.len()
}

/// Parse one `SKILL.md`. Two dialects, and the difference between them is the
/// answer to gh #304.
///
/// **Bounded** - the file opens with a `---` fence: everything up to the closing
/// fence is a header region, and the body starts after it. The fences make the
/// region unambiguous, so every line inside is chrome. An unterminated fence is not
/// a header, so the whole file is body.
///
/// **Headerless** - no opening fence: the leading run of *known* field lines is the
/// header, and the first line that is not one of the four starts the body. An
/// unknown key therefore never eats its own line.
///
/// The asymmetry is the fix. Dropping content is invisible; treating a header line
/// as prose merely shows it to the model, which a reader can see and correct. So the
/// safe reading of ambiguity is body - but only where the author did not fence the
/// region off and declare it a header.
pub fn parse_skill_text(fallback_name: &str, text: &str) -> ParsedSkill {
    let mut fields = Fields::new(fallback_name);
    let body_start = match opening_fence_end(text) {
        Some(after_open) => parse_bounded(text, after_open, &mut fields),
        None => parse_headerless(text, &mut fields),
    };
    let match_words = fields
        .match_line
        .split(',')
        .map(|word| word.trim().to_lowercase())
        .filter(|word| !word.is_empty())
        .collect();
    ParsedSkill {
        name: fields.name,
        description: fields.description,
        match_words,
        model_invocable: fields.model_invocable,
        body: text.get(body_start..).unwrap_or("").trim().to_string(),
    }
}
