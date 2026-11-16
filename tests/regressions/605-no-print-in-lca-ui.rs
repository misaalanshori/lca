//! Gh #605: no `print!`/`println!`/`eprintln!` in `lca-ui`'s production code.
//!
//! # The defect this closes
//!
//! `crates/lca-ui/src/chat_commands.rs` shipped
//! `eprintln!("OPEN_THINKING_WIZARD called")` inside `open_thinking_wizard`, from gh #397's
//! cycle (`e52657b7`) until this one - **every release in between**. Found by driving the
//! wizard in a real pane (`/settings` → type `effort` → Enter), not by any test: the frames
//! it paints are identical with the print present, so a unit row cannot see it. It only
//! shows on a real terminal's stderr, which is why it survived three weeks.
//!
//! # Why the class, not just the instance
//!
//! `crates/lca-ui` owns the interface the user is looking at, and it writes there through
//! the layered renderer, never through the process's stdio: a `println!` inside an
//! alt-screen frame corrupts it, and on a pty whose reader is gone it **panics** (the hazard
//! `crates/lca-ui/src/run_terminal.rs` documents - gh #403's pty row failed 101 on macOS
//! exactly that way). `lca-cli` is the opposite by design: it prints to stderr for headless
//! notices and `main`'s warnings. So the boundary is the crate boundary, and this row holds
//! it at `lca-ui` (`lca-tui` was checked in the same pass and is clean except its
//! `force_color_depth` warning, which is a deliberate one-line diagnostic on an invalid
//! `LCA_COLOR_DEPTH` - **not** covered here, and said so rather than silently).
//!
//! # The walk, and the two ways a guard like this reads as health
//!
//! * **One filename is not the module.** `chat_commands.rs` is one of ~90 files under
//!   `crates/lca-ui/src`, and an absence check over the file that had the defect would pass
//!   forever after a split moved the next print elsewhere. So the walk starts at the crate
//!   root and follows `mod` declarations through [`crate::module_sources`] (gh #551's
//!   helper, which honours `#[path]`, every visibility form, and skips `#[cfg(test)]`
//!   declarations).
//!
//! * **A comment about `eprintln!` is not code.** `run_terminal.rs` explains in prose why
//!   `println!` on a closed pty panics; a guard grepping raw text would find that sentence
//!   and either pass vacuously on a real print or fail on a comment. Comment lines are
//!   filtered first (the same rule as [`crate::code_lines`], applied **per line** here so the
//!   file's own line numbers survive - see `production_lines`' doc), so prose can neither
//!   satisfy nor break the row.
//!
//! # The stated limitation (`#[cfg(test)]`)
//!
//! The walk already skips a test module **declared** out of a production file
//! (`#[cfg(test)] mod tests;`, `#[cfg(test)] #[path = "chat_tests.rs"] mod …;`), because
//! `module_sources` honours the gate. What it cannot know is an **inline** `mod tests { … }`
//! body, which sits in the same file as the production code it tests. So this row tracks it
//! line by line: a `#[cfg(test)]` attribute marks the next `mod NAME {` it precedes, that
//! module's body is skipped until its braces close, and everything else is production.
//!
//! This is a heuristic, and it is stated rather than papered over: it counts braces after
//! removing string literals (so `println!("{{")` cannot open a phantom block), it does not
//! know a macro that expands to a `mod`, and a deliberately exotic layout - `#[cfg(test)]`
//! on an `impl` or a `fn` rather than a module - would be **skipped as production**, which
//! is a false negative. A false negative here costs a missed print; a false positive would
//! fail a healthy tree, and this shape prefers the first. The mutant (§4b of the brief) is
//! exactly this hole, and it is reported green as a coverage fact rather than hidden.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

/// The macros this crate must never write with: `print!` covers `print!`/`println!` because
/// the scan looks for the bare word, and `eprintln!` is listed separately for the message.
// Most specific first: `println!(` is a **substring** of `eprintln!(` (e·println!(), so a
// scan that tried `println!(` first would name the wrong macro on the very line this guard
// exists for - found while running mutant (a), which reported `println!( in
// eprintln!("OPEN_THINKING_WIZARD called")`.
const FORBIDDEN_MACROS: [&str; 3] = ["eprintln!(", "println!(", "print!("];

/// Strip string-literal bodies from a line, so a brace or a macro *inside* a string cannot
/// be mistaken for code.
///
/// Char-escapes are honoured (`\"` does not close the literal); a raw string (`r"…"`) is
/// treated as ordinary text, which can only make the brace count wrong on a line that also
/// holds a real string - and no such line exists in this crate today. Stated, per the
/// module doc's rule about limitations.
fn without_strings(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in line.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            continue;
        }
        out.push(ch);
    }
    out
}

/// Which forbidden macro, if any, `line` calls **in code** (not inside a string literal).
fn printed_macro(line: &str) -> Option<&'static str> {
    // Scan outside strings: an occurrence inside a literal is text the *program* will print
    // or hand to somewhere else, never the crate writing to its own stdout.
    let mut outside = String::with_capacity(line.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in line.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            outside.push(' ');
            continue;
        }
        if ch == '"' {
            in_string = true;
            outside.push(' ');
            continue;
        }
        outside.push(ch);
    }
    FORBIDDEN_MACROS.into_iter().find(|needle| {
        // The needle must not be preceded by an identifier character: `eprintln!(` after
        // `something_println!(` would otherwise match, and a macro whose name merely *ends*
        // in one of these words is not one of them.
        outside.match_indices(*needle).any(|(at, _)| {
            !outside[..at]
                .chars()
                .next_back()
                .is_some_and(|prev| prev.is_alphanumeric() || prev == '_')
        })
    })
}

/// The lines of `text` that are **production**, as `(the file's own line number, line)`.
///
/// Numbering is the file's, not the filtered text's: the first version of this guard
/// stripped comments with [`crate::code_lines`] *before* walking, which renumbered every
/// line and reported `chat_commands.rs:60` for a print on line 86. A message that sends a
/// reader to the wrong line is worse than no line number, so comments are filtered here
/// with their original index kept.
fn production_lines(text: &str) -> Vec<(usize, String)> {
    // `#[cfg(test)]` seen and not yet consumed by a `mod`; any other statement resets it
    // (matching `module_sources`' own rule, so the two walks cannot disagree about what an
    // attribute gates).
    let mut pending_test = false;
    // Brace depth at which the current inline test module opened, if one is open.
    let mut test_at_depth: Option<i32> = None;
    let mut depth = 0i32;
    let mut out = Vec::new();

    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let trimmed = line.trim();
        // A comment line is neither code nor test code: it can neither satisfy nor break
        // this row (gh #605 - `run_terminal.rs` explains `println!` in prose).
        if trimmed.starts_with("//") {
            continue;
        }
        let code = without_strings(line);
        let opens = code.matches('{').count() as i32;
        let closes = code.matches('}').count() as i32;

        if trimmed.starts_with("#[cfg(test)]") {
            pending_test = true;
            continue;
        }
        // An inline module body: `mod tests {`, whatever its visibility. `mod x;` declares a
        // file and is not an inline body - `module_sources` skipped it already if it was
        // gated, so a gated declaration reaching this loop is a test module it could not
        // follow.
        let inline_mod = {
            let rest = trimmed.strip_prefix("pub ").unwrap_or(trimmed);
            let rest = if let Some(after_pub) = rest.strip_prefix("pub(") {
                after_pub
                    .find(')')
                    .map(|close| after_pub[close + 1..].trim_start())
                    .unwrap_or(rest)
            } else {
                rest
            };
            rest.strip_prefix("mod ")
        };
        if let Some(rest) = inline_mod
            && rest.trim_end().ends_with('{')
        {
            depth += opens - closes;
            if pending_test {
                test_at_depth = Some(depth);
            }
            pending_test = false;
            if test_at_depth.is_none() {
                out.push((number, line.to_string()));
            }
            continue;
        }
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            pending_test = false;
        }

        if test_at_depth.is_none() {
            out.push((number, line.to_string()));
        }
        depth += opens - closes;
        if let Some(open_depth) = test_at_depth
            && depth < open_depth
        {
            test_at_depth = None;
        }
    }
    out
}

// Verifies: gh #605 - **no print macro reaches `lca-ui`'s production code.**
//
// The instance was one line in `chat_commands.rs`; the row walks every module of the crate
// from its root, so a print that a ceiling split or a refactor moves into a sibling is
// caught wherever it lands. Pre-cycle this row is RED on exactly the line the pane found
// (`chat_commands.rs:86`), which is what makes it a guard rather than a description.
#[test]
fn no_print_macro_in_lca_ui_outside_tests() {
    let mut offenders = Vec::new();
    for (path, text) in crate::module_sources("crates/lca-ui/src/lib.rs") {
        for (number, line) in production_lines(&text) {
            if let Some(mac) = printed_macro(&line) {
                offenders.push(format!("{path}:{number}: {mac} in {}", line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "`lca-ui` draws the interface through the renderer, never through the process's \
         stdio: a `print!`/`println!`/`eprintln!` here lands in the terminal the frame is \
         painted on (corrupting it, or panicking the process on a pty whose reader is gone \
         - the hazard `run_terminal.rs` documents), and it is invisible to every frame \
         assertion (gh #605). Found:\n{}\n\
         A legitimate diagnostic belongs in `lca-cli`, which owns stderr; test code belongs \
         behind `#[cfg(test)]`.",
        offenders.join("\n")
    );
}

// Verifies: gh #605 - **the walk reaches the whole crate, not the file that had the defect.**
//
// A guard whose scope is one filename passes forever after a split. This row asserts the
// walk is the crate: enough files to be the module tree, and at least one file from each of
// the areas where a stray print would be most tempting (the command surface, the renderer,
// the terminal loop). It is a scope assertion, not a count: the number may drift as the
// crate splits, the presence of those areas may not.
#[test]
fn the_print_walk_covers_the_whole_lca_ui_module_tree() {
    let sources = crate::module_sources("crates/lca-ui/src/lib.rs");
    assert!(
        sources.len() > 50,
        "the walk reached {} files; lca-ui's module tree is far larger than that, so this \
         guard is reading a fragment rather than the crate",
        sources.len()
    );
    for expected in [
        "crates/lca-ui/src/chat_commands.rs",
        "crates/lca-ui/src/chat_render.rs",
        "crates/lca-ui/src/run_terminal.rs",
        "crates/lca-ui/src/lib.rs",
    ] {
        assert!(
            sources.iter().any(|(path, _)| path == expected),
            "the walk must reach {expected}; it is one of the three places a stray print \
             would be least visible in a frame assertion"
        );
    }
    // And it excludes the test modules the crate declares, which is the half that makes the
    // exclusion honest rather than asserted: a test file contains print macros by design.
    assert!(
        !sources.iter().any(|(path, _)| path.ends_with("_tests.rs")),
        "a gated `#[cfg(test)]` module leaked into the production walk, so the exclusion is \
         not being applied and every test file's own prints would be judged as product code"
    );
}

// Verifies: gh #605 - **the exclusion is what the walk says it is**, driven directly on the
// two shapes it must distinguish rather than only inferred from the crate.
//
// An inline `mod tests { … }` body is test code even though it lives in the production
// file, and everything after its closing brace is production again. These are the two facts
// `production_lines` exists for; without them the row would either flag every inline test
// helper that prints a skip notice or miss a print outside one.
#[test]
fn an_inline_test_module_is_excluded_and_the_line_after_it_is_not() {
    let text = "\
fn prod_before() {}
#[cfg(test)]
mod tests {
    fn skip_notice() { eprintln!(\"skip: no git\"); }
}
fn prod_after() { println!(\"stray\"); }
";
    let lines = production_lines(text);
    let printed: Vec<&(usize, String)> = lines
        .iter()
        .filter(|(_, line)| printed_macro(line).is_some())
        .collect();
    assert_eq!(
        printed.len(),
        1,
        "exactly one production line prints: the one after the test module closed, found: {printed:?}"
    );
    assert!(
        printed[0].1.contains("stray"),
        "and it is the line outside the `#[cfg(test)]` module: {}",
        printed[0].1
    );
    // The test module's own print is not production code, and neither is its brace.
    assert!(
        !lines.iter().any(|(_, line)| line.contains("skip: no git")),
        "the inline test body is excluded, or the exclusion only applies to declared modules"
    );
}
