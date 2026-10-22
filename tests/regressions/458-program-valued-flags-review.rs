//! gh #458 - a flag whose value is a program (or a shell string) must review.
//!
//! These are the same class gh #347 drew the line at - code that is in no file -
//! reached through a *flag* instead of a positional argument. `vim -c ':!id' f`,
//! `ex -c '!id' f`, `ed f` and `tar xf a.tar --to-command=id` all auto-approved
//! in a trusted folder, as did four sibling `tar` flags the enumeration found
//! (`-I`/`--use-compress-program`, `-F`/`--info-script`/`--new-volume-script`,
//! `--rsh-command`, `--checkpoint-action=exec=`).
//!
//! The shape of the decision matters as much as the verdicts:
//!
//! 1. **`tar`'s flags are not "a program name"** - the value goes through a
//!    shell (`--to-command='id; echo PWNED'` runs both), so it is inline code and
//!    is refused on #347's rule. The set is *measured*, not taken from the
//!    issue's one example.
//! 2. **The editor `-c` family reviews wholesale**, because the argument is a
//!    mini-program (`:!`, `:r !`, `:g//!`, `:python`, `:terminal`) whose surface
//!    is a command language, not a set. The cost is scripted `vim -c` and it is
//!    stated, not discovered.
//! 3. **`ed` reviews structurally**: its entire command language arrives on
//!    stdin, so there is no argument to inspect and no configuration in which
//!    the analyzer could prove what runs.
//! 4. **`+cmd` is refused too**, because it is the same hole - measured,
//!    `vim '+!id' f` prints uid - so `reviewable_alternative` recommends `-S`
//!    (a script file in the folder), never `+wq`.
//! 5. **The benign neighbours stay scoped**, which is the false-positive cost
//!    made concrete: `vim file.txt`, `vim -S script.vim`, `tar xf a.tar`,
//!    `tar czf b.tgz dir`.
//!
//! Behaviour rows live in `crates/lca-permissions/src/shell_grammar.rs`
//! (`program_valued_flag_tests`, `reviewable_alternative_tests`) and
//! `crates/lca-permissions/tests/rules.rs`
//! (`program_valued_flags_review_in_a_trusted_folder`, the wiring row that goes
//! through the real `GrantStore`). This file pins what those cannot see from
//! outside: that the refusal set is the measured one, and that no future edit
//! quietly narrows it to the issue's single example.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

fn grammar() -> String {
    read("crates/lca-permissions/src/shell_grammar.rs")
}

/// The `tar` flag table, as written.
fn tar_flags() -> String {
    let source = grammar();
    let start = source
        .find("const TAR_PROGRAM_FLAGS")
        .expect("the tar flag table exists");
    let rest = &source[start..];
    let end = rest.find("];").expect("the table closes");
    rest[..end].to_string()
}

// Verifies: gh #458 - **all five** of tar's program-valued flags are in the
// table, not only the `--to-command` the issue named.
//
// The enumeration is the point: `-I`, `-F`/`--info-script`,
// `--new-volume-script`, `--rsh-command` run a program just as well, and leaving
// any of them out would have left the issue half-fixed while its title said
// otherwise.
#[test]
fn every_program_valued_tar_flag_is_measured() {
    let table = tar_flags();
    for flag in [
        "--to-command",
        "--use-compress-program",
        "--info-script",
        "--new-volume-script",
        "--rsh-command",
    ] {
        assert!(
            table.contains(flag),
            "the measured table carries {flag}: {table}"
        );
    }
}

// Verifies: gh #458 - the short spellings are handled by a character-wise scan,
// because `-I` can be glued (`-Iid`) and clustered (`cIf gzip`, and classic tar
// accepts flag clusters with no leading dash).
#[test]
fn the_short_program_flags_are_scanned_not_matched() {
    let source = grammar();
    let start = source
        .find("fn tar_program_flag(")
        .expect("the short-flag scanner exists");
    let body = &source[start..];
    let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
    assert!(
        body.contains("strip_prefix('-')"),
        "the dash is optional, as classic tar allows: {body}"
    );
    assert!(
        body.contains("'I'") && body.contains("'F'"),
        "both program-valued short flags are scanned: {body}"
    );
}

// Verifies: gh #458 - only the `exec=` checkpoint action runs a program.
// Measured: `exec=id` printed uid, `echo=hi` printed `hi`, `dot` printed `.`.
// A blanket rule on `--checkpoint-action` would refuse harmless progress output.
#[test]
fn the_checkpoint_action_is_narrowed_to_exec() {
    let source = grammar();
    let start = source
        .find("fn tar_program_flag(")
        .expect("the scanner exists");
    let body = &source[start..];
    let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
    assert!(
        body.contains("starts_with(\"exec=\")"),
        "the action must be exec=, not any action: {body}"
    );
}

// Verifies: gh #458 - the editor set covers the names that share vim's
// command-line surface, the way `mawk`/`nawk` are `awk` (gh #365's lesson).
#[test]
fn the_editor_set_covers_the_shared_names() {
    let source = grammar();
    let start = source
        .find("const EDITOR_COMMAND_FLAGS")
        .expect("the editor table exists");
    let rest = &source[start..];
    let table = &rest[..rest.find("];").expect("the table closes")];
    for name in ["\"vim\"", "\"vi\"", "\"view\"", "\"nvim\"", "\"ex\""] {
        assert!(table.contains(name), "{name} shares vim's surface: {table}");
    }
}

// Verifies: gh #458 - `+cmd` is refused with `-c`, because it is the *same*
// hole. Measured: `vim '+!id' f` prints uid.
//
// This is the row that pins the review copy's honesty: recommending `+wq` would
// move the hole into the advice, so `reviewable_alternative` must name `-S`.
#[test]
fn the_plus_command_form_is_refused_and_the_advice_does_not_recommend_it() {
    let source = grammar();
    let scanner_start = source
        .find("fn editor_command_flag(")
        .expect("the editor scanner exists");
    let body = &source[scanner_start..];
    let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
    assert!(
        body.contains("strip_prefix('+')"),
        "a bare `+cmd` is the same hole as `-c`: {body}"
    );

    // The advice recommends -S and explicitly warns against `+cmd`.
    let advice_start = source
        .find("pub fn reviewable_alternative(")
        .expect("the advice function exists");
    let advice = &source[advice_start..];
    let advice = &advice[..advice.find("\n}\n").unwrap_or(advice.len())];
    assert!(
        advice.contains("-S"),
        "the reviewable form is a script file: {advice}"
    );
    assert!(
        !advice.contains("use `+wq`") && !advice.contains("prefer `+"),
        "the advice must not recommend the +cmd spelling: {advice}"
    );
}

// Verifies: gh #458 - the wiring exists in the analyzer. The predicate's own
// rows call it directly and would all pass without this call, which is why the
// trusted-folder row in `tests/rules.rs` is the behavioural guard and this is
// the structural one.
#[test]
fn the_analyzer_actually_consults_the_rule() {
    let shell = read("crates/lca-permissions/src/shell.rs");
    assert!(
        shell.contains("program_valued_flag(&name"),
        "segment_ok calls the rule with the basenamed program"
    );
    assert!(
        !shell.contains("if false && program_valued_flag"),
        "and not behind a disabled condition"
    );
}

// Verifies: gh #458 - no new dependency; a flag scan needs no crate.
#[test]
fn no_new_dependency_was_added() {
    let manifest = read("crates/lca-permissions/Cargo.toml");
    // The crate's dependency block already carries what it needs (a scan over
    // `&[String]` and `char_indices`); nothing was added for this rule.
    for unwanted in ["regex", "globset", "nom", "pest"] {
        assert!(
            !manifest.contains(&format!("\n{unwanted} =")),
            "the flag scan is hand-written; no {unwanted} dependency: {manifest}"
        );
    }
}
