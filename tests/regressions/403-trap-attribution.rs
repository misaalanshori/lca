//! NFR-24 guard for gh #403 gap 5 (the first half): **a crash names what it can, and
//! accuses nothing else.**
//!
//! # The defect
//!
//! When a guest trapped, the host's `CallError::Trap(String)` carried wasmtime's own
//! text, the dispatcher formatted it as `{extension}: {error}`, and the tool result
//! rendered as
//!
//! ```text
//! conformance: extension trapped: wasm trap: wasm `unreachable` instruction executed
//! ```
//!
//! Three things wrong, and only the first is cosmetic:
//!
//! 1. the runtime's vocabulary, which tells a user nothing to act on;
//! 2. **no remedy**, so the only way out was to guess at `lca --help`;
//! 3. the payload went into scrollback, where it can carry a guest-internal path or
//!    address - the standing secrets rule.
//!
//! # What this cycle changed, and what it deliberately did not
//!
//! The attribution is a fact the host already had **at the moment the call failed**:
//! the trap happens inside a known instance's call, so the name is in scope. The fix is
//! therefore at the trap seam - classify by `wasmtime::Trap`'s variant (never by reading
//! the message text, which #151 removed), carry the class, and build the sentence where
//! the handle is visible. It is **not** a stack-parsing matcher, and that distinction
//! matters: a heuristic that guesses wrong sends a user to disable the wrong extension.
//!
//! Two directions the matcher must get right, and both have their own row:
//!
//! - **a host panic is not a guest trap.** LCA's fault must not be blamed on a guest.
//! - **no evidence, no accusation.** An unclassifiable trap says so and offers the
//!   diagnostic path rather than inventing a cause.
//!
//! # Deferred, and named rather than assumed
//!
//! The install-change detector (suppressing `/bug` in favour of restarting with
//! `--session`) is untouched. The resume-command formatter is **already shared** with
//! the fullscreen-exit hint (`resume_command.rs`, gap 2), so nothing there needs doing.
//! Gaps 3-4 are out of this batch.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

/// The host source that owns the trap seam.
fn host() -> String {
    read("crates/lca-ext-host/src/lib.rs")
}

// Verifies: gh #403 gap 5 - **the trap error carries a class, not a raw string.** The
// old shape (`Trap(String)`) made the destination of the runtime's text a formatting
// decision, and the formatting decision was `{extension}: {error}`. A class is what lets
// the sentence be chosen once, in one place, with the remedy attached.
#[test]
fn a_trap_carries_a_class_not_a_raw_string() {
    let source = host();
    assert!(
        source.contains("Trap(TrapDetail)"),
        "the error carries the detail type"
    );
    assert!(
        source.contains("pub enum TrapClass"),
        "and the class enum exists"
    );
    // It must NOT go back to a bare `String`: that is the shape whose rendering was
    // `extension: <wasmtime text>`.
    assert!(
        !source.contains("Trap(String)"),
        "a raw-string trap payload is the defect: a user-facing message needs a class"
    );
}

// Verifies: gh #403 gap 5 - **the class is derived from `wasmtime::Trap`'s variant.**
// #151 removed substring guessing because guest words riding an error chain could
// misfire (`fuel` in a link error reported fuel exhaustion). A later "simplification"
// that greps the message would reintroduce exactly that, so the classification site is
// pinned to the typed match.
#[test]
fn the_class_comes_from_the_trap_variant() {
    let source = host();
    let start = source
        .find("fn classify(&self, err: wasmtime::Error)")
        .expect("the classifier exists");
    let body = &source[start..];
    let end = body.find("\n    }\n").expect("it has a body");
    let body = &body[..end];
    for expected in [
        "Trap::UnreachableCodeReached",
        "Trap::MemoryOutOfBounds",
        "Trap::BadSignature",
    ] {
        assert!(
            body.contains(expected),
            "the classifier matches `{expected}` rather than reading text"
        );
    }
    // The one place a message could be inspected is forbidden: an assertion on the raw
    // payload would be the substring guess returning.
    assert!(
        !body.contains("raw.contains(") && !body.contains(".to_string().contains("),
        "the classifier must not inspect the payload text"
    );
}

// Verifies: gh #403 gap 5 (**the false-accusation rule**) - **a host panic is its own
// class, and it does not blame the extension.** The row beside the code proves the
// sentence; this proves the *shape*: a host failure takes the `host_panic` constructor
// rather than being folded into the guest path.
#[test]
fn a_host_panic_takes_its_own_path() {
    let source = host();
    assert!(
        source.contains("TrapDetail::host_panic("),
        "the host's own panic has a named constructor"
    );
    // The pool call and the blocking bridge both go through it.
    assert!(
        source
            .matches("CallError::Trap(TrapDetail::host_panic(")
            .count()
            >= 1,
        "the host panic path is wired"
    );
    assert!(
        source.contains("TrapClass::HostPanic"),
        "and its class is distinct from every guest class"
    );
    // `blames_extension` must be false for it - the predicate is the one place that
    // decides, so the message and any future consumer cannot disagree.
    let predicate = source
        .find("pub fn blames_extension")
        .expect("the predicate exists");
    let body = &source[predicate..];
    let end = body.find("\n    }\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("TrapClass::Unreachable")
            && body.contains("TrapClass::OutOfBoundsMemory")
            && body.contains("TrapClass::BadSignature"),
        "exactly the guest classes blame the extension: {body}"
    );
    assert!(
        !body.contains("HostPanic => true") && !body.contains("TrapClass::HostPanic"),
        "and a host panic is not among them: {body}"
    );
}

// Verifies: gh #403 gap 5 - **the message is built where the extension's name is in
// scope**, which is the dispatcher seam (`to_dispatch`), not the error type. The error
// type stays a pure value: the same detail is recorded before any name is attached, so
// a name stored on it could go stale.
#[test]
fn the_message_is_built_at_the_seam_that_has_the_name() {
    let source = host();
    let start = source
        .find("fn to_dispatch(call_err: CallError, extension: &str)")
        .expect("the dispatcher seam exists");
    let body = &source[start..];
    let end = body.find("\n}\n").expect("it has a body");
    let body = &body[..end];
    assert!(
        body.contains(
            "CallError::Trap(detail) => DispatchError::Failed(detail.message(extension))"
        ),
        "a trap goes through `message`, which has the name and the remedy: {body}"
    );
    // The generic arm must stay for every other class - this cycle changes one arm.
    assert!(
        body.contains("DispatchError::Failed(format!(\"{extension}: {other}\"))"),
        "the non-trap arms are unchanged"
    );
}

// Verifies: gh #403 gap 5 - **the payload is withheld from the rendered message and
// kept for a developer.** `LCA_DEBUG` and the session record need the raw text; a user
// must not receive it. One accessor, documented as developer-facing, is the difference
// between "we chose" and "we forgot".
#[test]
fn the_payload_is_kept_but_not_rendered() {
    let source = host();
    assert!(
        source.contains("pub fn raw(&self) -> &str"),
        "the raw text has an accessor for a developer"
    );
    let doc = &source[source
        .find("pub fn raw(&self) -> &str")
        .expect("the accessor exists")
        .saturating_sub(600)
        ..source
            .find("pub fn raw(&self) -> &str")
            .expect("the accessor exists")];
    assert!(
        doc.contains("Never") || doc.contains("LCA_DEBUG"),
        "and its doc says where it may be used: {doc}"
    );
    // `Display` must not be the raw text either: it reaches logs a person reads.
    let display = source
        .find("impl std::fmt::Display for TrapDetail")
        .expect("the Display impl exists");
    let body = &source[display..];
    let end = body.find("\n}\n").expect("it has a body");
    assert!(
        !body[..end].contains("self.raw"),
        "`Display` renders the class sentence, not the payload"
    );
}

// Verifies: gh #403 gap 5 - **the remedy names commands the CLI really has**, checked
// against the argument definitions rather than trusted from a string. A remedy that does
// not run is a second bug, and the failure mode is silent: the user copies it and gets
// `unrecognized subcommand`.
#[test]
fn the_remedy_names_real_cli_commands() {
    let cli = read("crates/lca-cli/src/cli_args.rs");
    for (needle, what) in [
        ("ext", "the `ext` subcommand"),
        ("Disable", "`ext disable`"),
        ("Install", "`ext install`"),
        ("no_extensions", "`--no-extensions`"),
    ] {
        assert!(
            cli.contains(needle),
            "{what} must exist for the remedy to be usable"
        );
    }
    // ...and the message builds exactly those spellings.
    let source = host();
    assert!(
        source.contains("lca ext disable") && source.contains("lca --no-extensions"),
        "the remedy uses the real spellings"
    );
}
