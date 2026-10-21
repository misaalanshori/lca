//! The `$EDITOR`/`$VISUAL` handoff (P6, FR-UI-15): open the buffer, run
//! the editor, and read it back through the editor contract.
//!
//! Split out of `hooks.rs` when that file crossed the 1,200-line ceiling.
//! The contract itself is the reason this is one module and not a function
//! inside a larger one: what an editor leaves on disk is not byte-for-byte
//! what the user typed, and gh #388 gap 4 is what it cost to learn.

/// Open `$EDITOR`/`$VISUAL` on the prompt text (P6).
pub(super) fn external_editor(text: &str) -> Option<String> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("EDITOR").ok().filter(|s| !s.is_empty()))?;
    external_editor_with(&editor, text, &std::env::temp_dir())
}

/// The testable core of [`external_editor`]: split the editor command on
/// shell words and spawn it directly with the buffer path as its own argv
/// element (#121). Paths with spaces need quoting nowhere: neither the
/// split nor the spawn rejoins words, and the buffer file carries a
/// unique name. An editor string that does not split (an unbalanced
/// quote) falls back to the platform shell — the one case that genuinely
/// needs one.
fn external_editor_with(editor: &str, text: &str, dir: &std::path::Path) -> Option<String> {
    let buffer = tempfile::Builder::new()
        .prefix("lca-prompt-")
        .suffix(".md")
        .tempfile_in(dir)
        .ok()?;
    // Close the handle before spawning: on Windows an open file cannot be
    // opened again by the editor. The path stays reserved (and deleted on
    // drop) through the guard below.
    let path = buffer.into_temp_path();
    std::fs::write(&path, text).ok()?;
    let status = match shell_words::split(editor) {
        Ok(words) => {
            let (program, args) = words.split_first()?;
            std::process::Command::new(program)
                .args(args)
                .arg(&path)
                .status()
                .ok()?
        }
        Err(_) => run_through_shell(&format!("{editor} {}", path.display())).ok()?,
    };
    // Read back through the editor contract, not verbatim: `None` here is
    // "the editor failed", and the caller keeps the prompt.
    let edited = std::fs::read_to_string(&path)
        .ok()
        .map(|text| normalize_edited_prompt(&text));
    status.success().then_some(edited).flatten()
}

/// The editor contract: what an editor leaves on disk is not byte-for-byte
/// what the user typed.
///
/// Every POSIX editor terminates the file it saves with a newline
/// (`vim`'s `fixendofline`, `nano`, `nvim` all do), so reading the buffer
/// back verbatim turns a one-line prompt into two lines, the second empty
/// — a blank line the user never typed at the end of every `$EDITOR` edit
/// (gh #388 gap 4). pi's rule is `stripBom(...).replace(/\n$/, "")`, and
/// this keeps the same narrowness deliberately:
///
/// - **One** terminator only, never `trim_end()`. A prompt that ends in a
///   blank line on purpose (a trailing paragraph, a `here:` line) keeps it;
///   `"protocol:\n- one\n\n"` becomes `"protocol:\n- one\n"`, which is
///   the blank line the user left, not the terminator the editor added.
/// - `\r\n` and a lone `\r` are terminators too, so a Windows editor's
///   CRLF does not leave a bare carriage return rendering as a control
///   character in the composer.
/// - A leading UTF-8 BOM is stripped: it is invisible in an editor and
///   would otherwise become a zero-width character at the start of the
///   message.
///
/// An empty buffer stays empty: the user cleared the prompt, which is an
/// edit, and distinct from a failed editor (`None`).
fn normalize_edited_prompt(raw: &str) -> String {
    let text = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    if let Some(rest) = text.strip_suffix("\r\n") {
        rest.to_string()
    } else if let Some(rest) = text.strip_suffix('\n') {
        rest.to_string()
    } else if let Some(rest) = text.strip_suffix('\r') {
        rest.to_string()
    } else {
        text.to_string()
    }
}

/// Run one command line through the platform shell: `sh -c` on Unix,
/// `cmd /C` on Windows. Only the fallback for an editor string no word
/// splitter accepts — everything else spawns directly.
fn run_through_shell(command: &str) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg(command)
            .status()
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::hooks::tests::python_on_path_for_tests;

    /// The python interpreter the fake editors are written in.
    fn python_on_path() -> String {
        python_on_path_for_tests()
    }

    /// A mock `$EDITOR`: appends a marker line to the file named by its
    /// first argument. Written into `dir` (which may contain spaces).
    fn mock_editor(dir: &std::path::Path) -> (String, std::path::PathBuf) {
        let script = dir.join("mock editor.py");
        std::fs::write(
            &script,
            "import sys\npath = sys.argv[1]\ntext = open(path, encoding=\"utf-8\").read()\nopen(path, \"w\", encoding=\"utf-8\").write(text + \"[edited]\\n\")\n",
        )
        .expect("write mock editor");
        let editor = format!("{} \"{}\"", python_on_path(), script.display());
        (editor, script)
    }

    /// A mock `$EDITOR` that writes **byte-exact** content to the buffer
    /// named by its first argument, then exits 0. Every real editor
    /// (`vim`, `nvim`, `nano`) terminates the file it saves with a
    /// newline, so the content passed here is the whole point of the
    /// test: it is what the buffer holds on return.
    fn editor_writing(dir: &std::path::Path, content: &str) -> String {
        editor_writing_bytes(dir, content.as_bytes())
    }

    /// [`editor_writing`] for content that is not valid UTF-8 — the editor
    /// is byte-oriented, so a test can hand it a buffer Rust cannot decode.
    fn editor_writing_bytes(dir: &std::path::Path, content: &[u8]) -> String {
        let script = dir.join("write editor.py");
        // The replacement is written as a repr so newlines, CRs and the
        // BOM survive as escapes rather than as literal source bytes.
        std::fs::write(
            &script,
            format!(
                "import sys\nopen(sys.argv[1], \"wb\").write({})\n",
                python_bytes_literal(content)
            ),
        )
        .expect("write mock editor");
        format!("{} \"{}\"", python_on_path(), script.display())
    }

    /// A mock `$EDITOR` that leaves the buffer exactly as it found it and
    /// exits 0 — the "saved without changes" case.
    fn editor_leaving_it_alone(dir: &std::path::Path) -> String {
        let script = dir.join("noop editor.py");
        std::fs::write(&script, "import sys\nsys.exit(0)\n").expect("write mock editor");
        format!("{} \"{}\"", python_on_path(), script.display())
    }

    /// A mock `$EDITOR` that fails, which must read as "no edit", never
    /// as "the user cleared the prompt".
    fn editor_that_fails(dir: &std::path::Path) -> String {
        let script = dir.join("failing editor.py");
        std::fs::write(&script, "import sys\nsys.exit(3)\n").expect("write mock editor");
        format!("{} \"{}\"", python_on_path(), script.display())
    }

    /// Render `content` as a Python bytes literal so a test can write exact
    /// bytes — a BOM, a CRLF, invalid UTF-8 — through the buffer.
    fn python_bytes_literal(content: &[u8]) -> String {
        let mut out = String::from("b\"");
        for byte in content {
            match byte {
                b'\\' => out.push_str("\\\\"),
                b'"' => out.push_str("\\\""),
                0x20..=0x7e => out.push(*byte as char),
                other => out.push_str(&format!("\\x{other:02x}")),
            }
        }
        out.push('"');
        out
    }

    // Verifies: #121 (the editor command splits on shell words, spawns
    // directly with the buffer path as its own argv element, and round
    // trips through a directory with spaces — on every platform).
    #[test]
    fn the_editor_launches_through_spaces_directly() {
        let root = lca_testkit::scratch_path("lca-editor-spaces");
        let dir = root.join("dir with spaces");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let (editor, _) = mock_editor(&dir);
        let edited = external_editor_with(&editor, "hello", &dir).expect("editor runs");
        // Exact, not `contains`: the mock appends `[edited]\n`, and since
        // gh #388 that terminator is the editor's, not the prompt's.
        assert_eq!(
            edited, "hello[edited]",
            "round trip through the spaced path, terminator dropped"
        );
    }

    /// Scratch dir for one editor round-trip row.
    fn editor_scratch(tag: &str) -> std::path::PathBuf {
        let dir = lca_testkit::scratch_path(tag);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    // Verifies: gh #388 gap 4 - the defect itself. Every POSIX editor
    // terminates the file it saves with a newline, so a one-line prompt
    // came back as two lines with an empty second one: open the editor to
    // fix a typo, get a message with a blank line you never typed.
    #[test]
    fn one_editor_terminator_never_becomes_a_blank_line() {
        let dir = editor_scratch("lca-editor-388-terminator");
        let editor = editor_writing(&dir, "fix the typo\n");
        let edited = external_editor_with(&editor, "fix teh typo", &dir).expect("editor runs");
        assert_eq!(
            edited, "fix the typo",
            "the file's own line terminator is not part of the prompt"
        );
    }

    // Verifies: gh #388 gap 4 - exactly one is stripped, not `trim_end`.
    // Two trailing newlines mean the user left one blank line on purpose.
    #[test]
    fn only_one_terminator_is_stripped() {
        let dir = editor_scratch("lca-editor-388-two");
        let editor = editor_writing(&dir, "line1\n\n");
        let edited = external_editor_with(&editor, "line1", &dir).expect("editor runs");
        assert_eq!(edited, "line1\n", "one of the two newlines was the user's");
    }

    // Verifies: gh #388 gap 4 - a deliberate trailing blank line survives.
    // This is the row that fails if the fix is `trim_end()`: a prompt
    // ending in `here:` followed by a blank line keeps that blank line.
    #[test]
    fn a_deliberate_trailing_blank_line_survives() {
        let dir = editor_scratch("lca-editor-388-deliberate");
        let editor = editor_writing(&dir, "protocol:\n- one\n- two\n\n");
        let edited = external_editor_with(&editor, "", &dir).expect("editor runs");
        assert_eq!(
            edited, "protocol:\n- one\n- two\n",
            "the blank line the user typed is content, not terminator"
        );
    }

    // Verifies: gh #388 gap 4 - a Windows editor writes CRLF, and the CR
    // must not survive as a stray control character in the composer.
    #[test]
    fn a_crlf_terminator_leaves_no_carriage_return() {
        let dir = editor_scratch("lca-editor-388-crlf");
        let editor = editor_writing(&dir, "line1\r\n");
        let edited = external_editor_with(&editor, "line1", &dir).expect("editor runs");
        assert_eq!(edited, "line1", "the CRLF pair is one terminator");
        assert!(!edited.contains('\r'), "and no bare CR is left behind");
    }

    // Verifies: gh #388 gap 4 - a lone CR is also a line terminator.
    #[test]
    fn a_lone_carriage_return_is_a_terminator() {
        let dir = editor_scratch("lca-editor-388-cr");
        let editor = editor_writing(&dir, "line1\r");
        let edited = external_editor_with(&editor, "line1", &dir).expect("editor runs");
        assert_eq!(edited, "line1", "an old-Mac terminator is still one");
    }

    // Verifies: gh #388 gap 4 (pi parity) - a UTF-8 BOM is invisible in an
    // editor and would otherwise become a zero-width character the user
    // never typed at the start of their message.
    #[test]
    fn a_byte_order_mark_is_stripped() {
        let dir = editor_scratch("lca-editor-388-bom");
        let editor = editor_writing(&dir, "\u{feff}hello\n");
        let edited = external_editor_with(&editor, "hello", &dir).expect("editor runs");
        assert_eq!(edited, "hello", "the BOM is an encoding artefact");
    }

    // Verifies: gh #388 gap 4 - a file holding only a BOM is empty, not
    // a prompt made of one invisible character.
    #[test]
    fn a_bom_only_buffer_is_empty() {
        let dir = editor_scratch("lca-editor-388-bom-only");
        let editor = editor_writing(&dir, "\u{feff}");
        let edited = external_editor_with(&editor, "hello", &dir);
        assert_eq!(
            edited.as_deref(),
            Some(""),
            "the user cleared the prompt; that is an edit, not a failure"
        );
    }

    // Verifies: gh #388 gap 4 - clearing the prompt is a *deliberate*
    // edit, and must be distinguishable from the editor failing. `None`
    // means "no edit" (the caller keeps the text); `Some("")` means the
    // user deleted everything (the caller clears it).
    #[test]
    fn clearing_the_prompt_is_not_the_same_as_the_editor_failing() {
        let dir = editor_scratch("lca-editor-388-cleared");
        let clearing = editor_writing(&dir, "");
        assert_eq!(
            external_editor_with(&clearing, "hello", &dir).as_deref(),
            Some(""),
            "an emptied buffer is a cleared prompt"
        );

        let failing = editor_that_fails(&dir);
        assert_eq!(
            external_editor_with(&failing, "hello", &dir),
            None,
            "a failed editor leaves the prompt alone"
        );
    }

    // Verifies: gh #388 gap 4 - an untouched save is still an edit, and
    // still loses the file's terminator.
    #[test]
    fn saving_without_changes_still_drops_the_terminator() {
        let dir = editor_scratch("lca-editor-388-unchanged");
        let editor = editor_leaving_it_alone(&dir);
        let edited = external_editor_with(&editor, "unchanged\n", &dir);
        assert_eq!(edited.as_deref(), Some("unchanged"), "read back normalized");
    }

    // Verifies: gh #388 gap 4 (fail closed) - invalid UTF-8 is a refused
    // edit, never a lossily mangled prompt. A silently corrupted message
    // is worse than one the user has to retype.
    #[test]
    fn invalid_utf8_is_a_failed_edit_not_a_mangled_one() {
        let dir = editor_scratch("lca-editor-388-utf8");
        // 0xFF is never valid UTF-8, in any position.
        let editor = editor_writing_bytes(&dir, b"hello \xff world\n");
        assert_eq!(
            external_editor_with(&editor, "hello", &dir),
            None,
            "undecodable bytes must fail closed"
        );
    }

    // Verifies: #121 (a plain `program --flag` editor splits the same way).
    #[test]
    fn the_editor_splits_flags_from_the_program() {
        let root = lca_testkit::scratch_path("lca-editor-flags");
        std::fs::create_dir_all(&root).expect("mkdir");
        let script = root.join("edit.py");
        std::fs::write(
            &script,
            "import sys\npath = sys.argv[1]\nopen(path, \"w\", encoding=\"utf-8\").write(\"flagged\\n\")\n",
        )
        .expect("write mock editor");
        let editor = format!("{} \"{}\"", python_on_path(), script.display());
        let edited = external_editor_with(&editor, "hello", &root).expect("editor runs");
        assert!(
            edited.contains("flagged"),
            "program and flag split, path appended: {edited:?}"
        );
    }
}
