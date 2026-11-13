//! The paste ladder's file tier (gh #403 gap 3): turning a clipboard's copied
//! file paths into the text a composer should receive.
//!
//! Pi's `handleClipboardPaste` probes **files -> image -> text**. This module is
//! the first tier's formatting half, kept pure so every rule can be asserted
//! without a clipboard: a real one cannot be driven in CI (no X11 owner, no
//! pasteboard, no win32 session), and the rules are the actual feature.
//!
//! The rules, from pi (`../pistudy/038-*` §2.1):
//!
//! * a path containing a **control character** refuses the whole paste - a
//!   `\n` inside a filename would otherwise let a copied name inject a second
//!   line into the composer, and a `\u{1b}` could reach the renderer;
//! * in `!` bash mode each path is **shell-quoted** and the paths are joined
//!   with spaces, so `!cat a b` works and a name with a space survives as one
//!   argument (`'my file.txt'`, with pi's `'\''` escaping inside single quotes);
//! * in prompt mode paths are **newline-joined**, so each one reads as its own
//!   line and an `@path` opener on a line of its own does not swallow its
//!   neighbour;
//! * exactly **one** space separates the insertion from what is already there,
//!   when either side needs it (pi's "smart surrounding spaces").

use std::path::PathBuf;

/// Which composer the paste lands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteMode {
    /// The ordinary prompt: paths are newline-joined.
    Prompt,
    /// `!` bash mode: each path is shell-quoted and the paths are space-joined.
    Bash,
}

/// Why a copied file list could not be turned into text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteFilesError {
    /// The offending path, as copied (so the notice can name it).
    pub path: String,
}

impl std::fmt::Display for PasteFilesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "a copied file name contains a control character: {:?}",
            self.path
        )
    }
}

/// The text one paste of `paths` inserts, or the refusal.
///
/// `needs_leading_space` / `needs_trailing_space` are the caller's reading of the
/// composer's surroundings (the character before and after the caret); pi inserts
/// a space on either side that abuts non-whitespace, so a paste never welds itself
/// onto a neighbouring word. Exactly one space, never two.
pub fn files_insertion(
    paths: &[PathBuf],
    mode: PasteMode,
    needs_leading_space: bool,
    needs_trailing_space: bool,
) -> Result<Option<String>, PasteFilesError> {
    // An empty file list is not a paste: the clipboard advertised a file
    // list and it was empty, which means "no files", so the caller falls
    // through to the image/text tiers rather than inserting a stray space.
    if paths.is_empty() {
        return Ok(None);
    }
    let mut parts = Vec::with_capacity(paths.len());
    for path in paths {
        let text = path.to_string_lossy().into_owned();
        // A control character in a copied name is refused outright: the rest of
        // the list is not pasted either, because a partial paste of a multi-file
        // copy is more confusing than a refusal that names the offender.
        if text.chars().any(char::is_control) {
            return Err(PasteFilesError { path: text });
        }
        parts.push(match mode {
            PasteMode::Bash => shell_quote(&text),
            PasteMode::Prompt => text,
        });
    }
    let joined = match mode {
        // A space between arguments: the shell splits on it and each already-quoted
        // path stays one argument.
        PasteMode::Bash => parts.join(" "),
        PasteMode::Prompt => parts.join("\n"),
    };
    let mut out = String::with_capacity(joined.len() + 2);
    if needs_leading_space {
        out.push(' ');
    }
    out.push_str(&joined);
    if needs_trailing_space {
        out.push(' ');
    }
    Ok(Some(out))
}

/// Which tier a paste should use, given everything the clipboard offered.
///
/// Pi's ladder is **files -> image -> text** and it is a *priority*, not a set of
/// fallbacks: a clipboard that holds both a file list and a bitmap pastes the
/// **files** - that is what the user copied, and the bitmap is usually a
/// screenshot of the same thing. This function is the whole decision, pure, so a
/// row can put all three tiers in one call and assert the order rather than
/// trusting the shape of a chain of `if let`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteTier {
    /// The first tier: copied file paths (already formatted for the composer).
    Files(String),
    /// The second tier: a bitmap, which the caller stages as an attachment.
    Image,
    /// The third tier: plain text, inserted as-is.
    Text(String),
    /// Nothing the clipboard could give.
    Nothing,
}

/// Pick the tier. `files` is the file tier's outcome - `Some(Ok(paths))` when the
/// clipboard had a list, `Some(Err(refusal))` when a name was refused (which
/// **ends** the ladder: a partial paste would be worse than none), `None` when
/// it had none.
pub fn pick_tier(
    files: Option<Result<Vec<PathBuf>, String>>,
    has_image: bool,
    text: Option<String>,
) -> PasteTier {
    match files {
        Some(Ok(paths)) if !paths.is_empty() => {
            match files_insertion(&paths, PasteMode::Prompt, false, false) {
                Ok(Some(inserted)) => PasteTier::Files(inserted),
                // An empty list cannot reach here, but the two remaining arms are
                // the same answer pi gives: fall through.
                _ => fall_through(has_image, text),
            }
        }
        Some(Err(refusal)) => PasteTier::Files(refusal),
        _ => fall_through(has_image, text),
    }
}

fn fall_through(has_image: bool, text: Option<String>) -> PasteTier {
    if has_image {
        PasteTier::Image
    } else {
        match text {
            Some(text) if !text.is_empty() => PasteTier::Text(text),
            _ => PasteTier::Nothing,
        }
    }
}

/// Quote one path for a POSIX shell, only when it needs it (pi's `quoteIfNeeded`).
///
/// A bare word stays bare so `!cat a.txt` reads naturally; anything with a space,
/// a quote, or a shell metacharacter is single-quoted, with an embedded `'`
/// written as `'\''` (`'` closes, `\'` is a literal quote, `'` reopens).
fn shell_quote(value: &str) -> String {
    let needs = value.chars().any(|c| {
        !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '@' | '%'))
    });
    if !needs {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(items: &[&str]) -> Vec<PathBuf> {
        items.iter().map(PathBuf::from).collect()
    }

    // Verifies: gh #403 gap 3 - one path, prompt mode: inserted as itself, no
    // surrounding spaces when the composer's ends are already whitespace.
    #[test]
    fn one_path_in_prompt_mode_inserts_its_own_text() {
        let inserted = files_insertion(
            &paths(&["/tmp/report.pdf"]),
            PasteMode::Prompt,
            false,
            false,
        )
        .expect("a clean path")
        .expect("one path");
        assert_eq!(inserted, "/tmp/report.pdf");
    }

    // Verifies: gh #403 gap 3 - several paths are **newline-joined** in prompt
    // mode, so each reads as its own line and an `@path` cannot swallow the next.
    #[test]
    fn several_paths_in_prompt_mode_are_newline_joined() {
        let inserted = files_insertion(
            &paths(&["/a/one.txt", "/a/two.txt"]),
            PasteMode::Prompt,
            false,
            false,
        )
        .expect("clean paths")
        .expect("two paths");
        assert_eq!(inserted, "/a/one.txt\n/a/two.txt");
        assert_eq!(inserted.matches('\n').count(), 1, "exactly one separator");
    }

    // Verifies: gh #403 gap 3 - `!` bash mode **shell-quotes** each path and joins
    // with spaces, so `!cat 'my file.txt' other.txt` is two arguments, not three.
    #[test]
    fn bash_mode_quotes_a_path_with_a_space_and_space_joins() {
        let inserted = files_insertion(
            &paths(&["/tmp/my file.txt", "/tmp/other.txt"]),
            PasteMode::Bash,
            false,
            false,
        )
        .expect("clean paths")
        .expect("two paths");
        assert_eq!(inserted, "'/tmp/my file.txt' /tmp/other.txt");
        assert!(!inserted.contains('\n'), "bash mode never splits a command");
    }

    // Verifies: gh #403 gap 3 - a bare path stays unquoted in bash mode, so the
    // common case reads the way a user typed it.
    #[test]
    fn bash_mode_leaves_a_plain_path_unquoted() {
        let inserted = files_insertion(&paths(&["/tmp/a.txt"]), PasteMode::Bash, false, false)
            .expect("a clean path")
            .expect("one path");
        assert_eq!(inserted, "/tmp/a.txt", "no quotes around a bare word");
    }

    // Verifies: gh #403 gap 3 - an embedded single quote is escaped the POSIX way
    // (`'\\''`), so a name containing `'` survives as one argument.
    #[test]
    fn bash_mode_escapes_an_embedded_quote() {
        let inserted = files_insertion(
            &paths(&["/tmp/it's here.txt"]),
            PasteMode::Bash,
            false,
            false,
        )
        .expect("clean path")
        .expect("one path");
        assert_eq!(inserted, "'/tmp/it'\\''s here.txt'");
    }

    // Verifies: gh #403 gap 3 - **a control character refuses the whole paste**,
    // naming the offending path. A copied `\n` would otherwise inject a second
    // composer line, and an ESC could reach the renderer.
    #[test]
    fn a_path_with_a_control_character_is_refused() {
        for bad in ["/tmp/a\nb.txt", "/tmp/a\u{1b}[31mb", "/tmp/a\u{7}.txt"] {
            let err = files_insertion(
                &paths(&["/tmp/fine.txt", bad]),
                PasteMode::Prompt,
                false,
                false,
            )
            .expect_err("a control character is refused");
            assert_eq!(err.path, bad, "the refusal names the path");
            assert!(
                err.to_string().contains("control character"),
                "and says what is wrong: {err}"
            );
        }
    }

    // Verifies: gh #403 gap 3 - pi's **smart surrounding spaces**: one space on
    // either side that abuts a non-whitespace character, so the paste never welds
    // itself to a neighbouring word, and never two.
    #[test]
    fn smart_surrounding_spaces_add_exactly_one_each() {
        let inserted = files_insertion(
            &paths(&["/tmp/a.txt"]),
            PasteMode::Prompt,
            true, // the composer has text right before the caret
            true, // and right after it
        )
        .expect("clean path")
        .expect("one path");
        assert_eq!(inserted, " /tmp/a.txt ", "one space each side, not two");

        let leading_only = files_insertion(&paths(&["/tmp/a.txt"]), PasteMode::Prompt, true, false)
            .expect("clean path")
            .expect("one path");
        assert_eq!(leading_only, " /tmp/a.txt");
        let trailing_only =
            files_insertion(&paths(&["/tmp/a.txt"]), PasteMode::Prompt, false, true)
                .expect("clean path")
                .expect("one path");
        assert_eq!(trailing_only, "/tmp/a.txt ");
    }

    // Verifies: gh #403 gap 3 - **the ladder's order**: files beat an image and
    // beat text, all three present in one call. pi's rule is a priority, not a
    // fallback chain, so a clipboard holding both a file list and a screenshot
    // pastes the file list - what the user actually copied.
    #[test]
    fn files_outrank_an_image_and_text() {
        let tier = pick_tier(
            Some(Ok(paths(&["/tmp/a.txt"]))),
            true,
            Some("clipboard prose".to_string()),
        );
        assert_eq!(
            tier,
            PasteTier::Files("/tmp/a.txt".to_string()),
            "the file tier wins when all three are available"
        );
        // No files: the image wins over text, exactly as before this cycle.
        assert_eq!(
            pick_tier(None, true, Some("prose".to_string())),
            PasteTier::Image
        );
        // No files, no image: text, and then nothing.
        assert_eq!(
            pick_tier(None, false, Some("prose".to_string())),
            PasteTier::Text("prose".to_string())
        );
        assert_eq!(pick_tier(None, false, None), PasteTier::Nothing);
    }

    // Verifies: gh #403 gap 3 - a **refused** name ends the ladder rather than
    // falling through: pasting the text tier after refusing a file list would
    // hand the user a path string they cannot use, with no notice saying why.
    #[test]
    fn a_refused_name_ends_the_ladder() {
        let tier = pick_tier(
            Some(Err(
                "paste refused: a copied file name contains a control character: \"/tmp/a\nb\""
                    .to_string(),
            )),
            true,
            Some("prose".to_string()),
        );
        let PasteTier::Files(notice) = tier else {
            panic!("a refusal must be shown, not swallowed into another tier");
        };
        assert!(
            notice.starts_with("paste refused:") && notice.contains("control character"),
            "the notice says it was refused and why: {notice}"
        );
    }

    // Verifies: gh #403 gap 3 - an **empty** file list is not a paste: the caller
    // falls through to the lower tiers, and an empty banner on the clipboard must
    // not insert an empty newline.
    #[test]
    fn an_empty_list_inserts_nothing_that_matters() {
        let inserted =
            files_insertion(&[], PasteMode::Prompt, true, true).expect("nothing to refuse");
        assert_eq!(
            inserted, None,
            "the caller must treat an empty list as `no files`, not as an insertion"
        );
    }
}
