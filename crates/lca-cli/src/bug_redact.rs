//! Secret-shaped string redaction for the diagnostic bundle (gh #411 gap 3).
//!
//! `/bug`'s trust foundation (gh #57) is that the credential *store* never enters
//! the bundle. But a key the user **pasted into the chat** lives in
//! `session.jsonl` raw, and that bundle is designed to be shared - so "no secrets"
//! meant "no *stored* secrets", which is a weaker claim than the docs make.
//!
//! # The rule this table follows
//!
//! **Err toward scrubbing.** A false redaction costs a maintainer one round-trip
//! (they ask, the user re-sends, or the surrounding context is enough). A missed
//! secret is a leaked credential. So the patterns are deliberately loose, each
//! ambiguous match is *counted* rather than silently dropped, and there is no
//! precision-tuning pass. The report says how many and of what kind, never the
//! value - not even truncated, because a truncated key is still a key.
//!
//! # What it does not try to be
//!
//! It is not a credential detector for arbitrary prose. It finds the shapes that
//! credential issuers actually mint (a known prefix plus a long opaque run), and
//! it finds an opaque value sitting next to a credential-ish word. A secret with
//! no recognisable shape and no label around it - a bare password in a sentence -
//! is out of reach, and pretending otherwise would be the worst kind of false
//! assurance. `docs/usage.md` says so in the user's own words.

/// One family of credential shape, with a count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scrubbed {
    /// One entry per family that matched: `(kind, count)`.
    families: Vec<(&'static str, usize)>,
}

impl Scrubbed {
    /// The total number of scrubbed strings.
    pub fn total(&self) -> usize {
        self.families.iter().map(|(_, count)| count).sum()
    }

    /// The families that matched, in table order.
    pub fn families(&self) -> &[(&'static str, usize)] {
        &self.families
    }

    /// The line the inspection screen and the report carry: counts and kinds,
    /// never values.
    pub fn summary(&self) -> String {
        if self.total() == 0 {
            return "scrubbed nothing (no secret-shaped strings found)".to_string();
        }
        let parts: Vec<String> = self
            .families
            .iter()
            .map(|(kind, count)| {
                if *count == 1 {
                    format!("1 {kind}")
                } else {
                    format!("{count} {kind}s")
                }
            })
            .collect();
        format!(
            "scrubbed {} secret-shaped string{}: {}",
            self.total(),
            if self.total() == 1 { "" } else { "s" },
            parts.join(", ")
        )
    }

    /// Add one family's count (also used to merge two passes).
    pub(crate) fn add(&mut self, family: &'static str, count: usize) {
        if count == 0 {
            return;
        }
        match self.families.iter_mut().find(|(kind, _)| *kind == family) {
            Some((_, existing)) => *existing += count,
            None => self.families.push((family, count)),
        }
    }
}

/// The replacement text. Fixed rather than empty so a maintainer reading the
/// bundle can see *where* something was removed, and so the message stays
/// readable (`my key is <redacted> please fix it`).
pub const REDACTED: &str = "<redacted>";

/// One pattern family. `find` returns the byte ranges to replace.
struct Family {
    kind: &'static str,
    /// The token prefixes that identify a credential unequivocally.
    prefixes: &'static [&'static str],
    /// The minimum length of the opaque run that follows the prefix.
    min_tail: usize,
}

/// The credential families, chosen from what issuers actually mint.
///
/// Every entry is "a known prefix plus an opaque run", which is what makes it
/// safe to scrub aggressively: the prefix is the evidence, so the run after it is
/// a secret by construction.
const FAMILIES: &[Family] = &[
    Family {
        kind: "API key",
        prefixes: &["sk-", "sk_", "rk-", "pk-"],
        min_tail: 16,
    },
    Family {
        kind: "Anthropic key",
        prefixes: &["sk-ant-"],
        min_tail: 16,
    },
    Family {
        kind: "GitHub token",
        prefixes: &["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"],
        min_tail: 16,
    },
    Family {
        kind: "Slack token",
        prefixes: &["xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-"],
        min_tail: 10,
    },
    Family {
        kind: "AWS key id",
        prefixes: &["AKIA", "ASIA", "AGPA", "AIDA"],
        min_tail: 12,
    },
    Family {
        kind: "Google key",
        prefixes: &["AIza"],
        min_tail: 20,
    },
    Family {
        kind: "bearer token",
        prefixes: &["Bearer ", "bearer "],
        min_tail: 16,
    },
    Family {
        kind: "JWT",
        prefixes: &["eyJ"],
        min_tail: 24,
    },
    Family {
        kind: "OAuth code",
        prefixes: &["code=", "access_token=", "refresh_token=", "id_token="],
        min_tail: 12,
    },
];

/// Whether `ch` can appear inside an opaque credential run.
fn is_opaque(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~' | '+' | '/' | '=')
}

/// The words that mark the value beside them as a credential.
///
/// Matching one of these is what lets an unlabelled opaque run - a bare hex or
/// base64 blob - be scrubbed: the *label* is the evidence, not the shape. It
/// matches inside a longer identifier too (`api_key`, `apiKey`, `x-api-key`), so a
/// key written in any of those forms is caught.
const LABELS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "secret",
    "token",
    "password",
    "passwd",
    "credential",
    "authorization",
    "auth",
    "cookie",
];

/// Minimum length of an opaque run for it to be worth scrubbing beside a label.
/// Short enough to catch a real secret, long enough that a word or a small hex
/// number is not eaten.
const LABELLED_RUN_MIN: usize = 16;

/// Redact every recognised secret shape in `text`, returning the new text and
/// what was scrubbed.
///
/// Structure is preserved: only the matched run becomes [`REDACTED`]. Roles,
/// timestamps, tool names, and the surrounding sentences are untouched, because a
/// bundle a maintainer cannot read is no better than no bundle.
pub fn redact(text: &str) -> (String, Scrubbed) {
    let mut scrubbed = Scrubbed::default();
    let mut out = String::with_capacity(text.len());
    // Work on characters so a multi-byte grapheme is never split, and so the
    // opaque-run scan cannot land mid-codepoint.
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        if let Some((end, kind)) = prefix_match(&chars, index) {
            out.push_str(REDACTED);
            scrubbed.add(kind, 1);
            index = end;
            continue;
        }
        if let Some((end, kind)) = labelled_match(&chars, index) {
            // Keep the label, replace the value: `api_key: <redacted>` reads.
            out.extend(&chars[index..end.label_end]);
            out.push_str(REDACTED);
            scrubbed.add(kind, 1);
            index = end.value_end;
            continue;
        }
        out.push(chars[index]);
        index += 1;
    }

    (out, scrubbed)
}

/// If a credential prefix starts at `index`, the end of the run to replace.
fn prefix_match(chars: &[char], index: usize) -> Option<(usize, &'static str)> {
    let rest: String = chars[index..].iter().take(24).collect();
    for family in FAMILIES {
        for prefix in family.prefixes {
            if !rest.starts_with(prefix) {
                continue;
            }
            let mut end = index + prefix.chars().count();
            while end < chars.len() && is_opaque(chars[end]) {
                end += 1;
            }
            // The run has to be long enough to be a secret rather than an ordinary
            // word that happens to start with the same letters (`sk-` alone, or
            // `code=` with a short value in prose).
            let tail = end - (index + prefix.chars().count());
            if tail >= family.min_tail {
                return Some((end, family.kind));
            }
        }
    }
    None
}

/// The end of a labelled value.
struct Labelled {
    /// Where the label itself ends (kept in the output).
    label_end: usize,
    /// Where the value ends (replaced).
    value_end: usize,
}

/// If a credential label sits at `index` followed by an opaque run, the span.
fn labelled_match(chars: &[char], index: usize) -> Option<(Labelled, &'static str)> {
    // A label only counts at a word boundary, so `mytokens` is not `token`.
    let previous = index.checked_sub(1).and_then(|at| chars.get(at));
    if previous.is_some_and(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '-') {
        return None;
    }
    let rest: String = chars[index..].iter().take(40).collect();
    let lower = rest.to_ascii_lowercase();
    let label = LABELS.iter().find(|label| lower.starts_with(*label))?;
    let mut cursor = index + label.chars().count();
    // Skip the separators between a label and its value: `=`, `:`, space, quotes.
    let mut saw_separator = false;
    while cursor < chars.len() && matches!(chars[cursor], '=' | ':' | ' ' | '"' | '\'' | '\t') {
        saw_separator = true;
        cursor += 1;
    }
    if !saw_separator {
        return None;
    }
    let value_start = cursor;
    while cursor < chars.len() && is_opaque(chars[cursor]) {
        cursor += 1;
    }
    let run = cursor - value_start;
    // Beside a label the run only has to be plausibly secret-shaped; a bare word
    // (`token = none`) stays readable, an opaque blob does not.
    if run < LABELLED_RUN_MIN || !looks_opaque(&chars[value_start..cursor]) {
        return None;
    }
    Some((
        Labelled {
            label_end: value_start,
            value_end: cursor,
        },
        "labelled credential",
    ))
}

/// Whether a run is opaque enough to be a credential rather than a word: it has to
/// mix letters and digits (or be mostly hex/base64), so `authorization = bearer`
/// and `token = expired` survive while a real blob does not.
fn looks_opaque(run: &[char]) -> bool {
    let digits = run.iter().filter(|ch| ch.is_ascii_digit()).count();
    let letters = run.iter().filter(|ch| ch.is_ascii_alphabetic()).count();
    if digits == 0 || letters == 0 {
        return false;
    }
    // A credential run is mostly opaque characters, not spaces or prose.
    let opaque = run.iter().filter(|ch| is_opaque(**ch)).count();
    opaque * 10 >= run.len() * 9
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: gh #411 design point 3 - the surrounding text survives, so the
    // bundle stays debuggable.
    #[test]
    fn redaction_keeps_the_sentence_around_the_secret() {
        let (out, scrubbed) = redact("my key is sk-test-DONOTUSE-abcdefghijklmnop and it broke");
        assert_eq!(
            out, "my key is <redacted> and it broke",
            "only the secret is replaced"
        );
        assert_eq!(scrubbed.total(), 1);
    }

    // Verifies: gh #411 - an ordinary word that shares a credential prefix's
    // letters is not eaten.
    #[test]
    fn ordinary_words_are_not_scrubbed() {
        for clean in [
            "the code= is missing",
            "sk- alone is not a key",
            "token = expired",
            "authorization = bearer",
            "ask-me-anything",
            "a task list",
        ] {
            let (out, scrubbed) = redact(clean);
            assert_eq!(out, clean, "nothing to scrub in {clean:?}");
            assert_eq!(scrubbed.total(), 0, "and nothing counted for {clean:?}");
        }
    }

    // Verifies: gh #411 design point 2 - the summary reports kinds and counts,
    // never values.
    #[test]
    fn the_summary_names_kinds_not_values() {
        let (_, scrubbed) = redact(
            "a sk-test-DONOTUSE-abcdefghijklmnop b ghp_DONOTUSE0000000000000000000000000000",
        );
        let summary = scrubbed.summary();
        assert!(summary.contains("scrubbed 2"), "{summary}");
        assert!(!summary.contains("DONOTUSE"), "{summary}");
        assert!(!summary.contains("sk-test"), "{summary}");
    }

    // Verifies: gh #411 - a clean transcript says so, rather than staying silent.
    #[test]
    fn a_clean_text_reports_nothing_scrubbed() {
        let (out, scrubbed) = redact("nothing to see, just a stack trace");
        assert_eq!(out, "nothing to see, just a stack trace");
        assert!(
            scrubbed.summary().contains("nothing"),
            "{}",
            scrubbed.summary()
        );
    }
}
