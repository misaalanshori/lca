//! gh #411 gaps 3 and 2: the diagnostic bundle is scrubbed before it is written,
//! and the user inspects it first.
//!
//! `/bug`'s trust foundation (gh #57) is that the credential *store* never enters
//! the bundle. But a key the user **pasted into the chat** lives in
//! `session.jsonl` raw, and that bundle is designed to be shared - so "no secrets"
//! meant "no *stored* secrets", a weaker claim than the docs imply.
//!
//! Every row here uses an obviously fake planted value (`sk-test-DONOTUSE-...`);
//! this is the cycle whose subject is secret hygiene, so the fixtures carry no real
//! credential.
//!
//! These are the NFR-24 rows for the defect (gh #411 gaps 3 and 2). They live here
//! rather than in `crates/lca-cli/tests/` because a shipped defect gets a named
//! guard, and the redaction table is exactly the kind of thing a later
//! "simplification" would quietly thin out.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::{Path, PathBuf};

use lca_cli::bug;

/// An obviously fake key. Never a real credential: the whole point of the fixture
/// is that it is recognisable as a test value in a bundle, a log, or a report.
const PLANTED: &str = "sk-test-DONOTUSE-000000000000000000000000";

fn scratch(name: &str) -> PathBuf {
    let root = lca_testkit::scratch_path(&format!("lca-bug411-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("project")).expect("mkdir project");
    std::fs::create_dir_all(root.join("data")).expect("mkdir data");
    root
}

/// A session with `content` as its only user turn, returning its id.
fn session_with(root: &Path, content: &str) -> String {
    let store = lca_session::SessionStore::new(root.join("data"));
    let session = store
        .create_session(&root.join("project"), "bug411")
        .expect("session");
    store
        .append(
            &session,
            lca_protocol::Record::User {
                v: 1,
                ts: 2,
                id: "u1".to_string(),
                parent: None,
                content: content.to_string(),
                attachments: Vec::new(),
                queue: None,
            },
        )
        .expect("append");
    session.id().to_string()
}

/// The bytes of a file inside a built bundle.
fn entry(bundle: &Path, name: &str) -> String {
    let file = std::fs::File::open(bundle).expect("open bundle");
    let mut archive = zip::ZipArchive::new(file).expect("zip");
    let mut text = String::new();
    std::io::Read::read_to_string(&mut archive.by_name(name).expect("entry"), &mut text)
        .expect("read entry");
    text
}

/// The on-disk session log, verbatim.
fn on_disk(root: &Path) -> String {
    let log = find_log(&root.join("data")).expect("a session log on disk");
    std::fs::read_to_string(log).expect("read log")
}

fn find_log(dir: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_log(&path) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|name| name == "log.jsonl") {
            return Some(path);
        }
    }
    None
}

// ------------------------------------------------------------------ the defect

// Verifies: gh #411 gap 3 - the defect. A key pasted into the chat does **not**
// reach the bundle.
#[test]
fn a_pasted_key_never_reaches_the_bundle() {
    let root = scratch("pasted");
    let id = session_with(&root, &format!("here is my key {PLANTED} please fix it"));
    let plan = bug::stage(&root.join("data"), &root.join("project"), Some(&id), &root)
        .expect("stage the bundle");
    let written = bug::write(&plan).expect("write the bundle");

    let transcript = entry(&written.path, "session.jsonl");
    assert!(
        !transcript.contains(PLANTED),
        "the pasted key never reaches the bundle:\n{transcript}"
    );
    // The rest of the message survives, so the bundle stays debuggable. A scrub
    // that deletes the whole line is not a redaction, it is data loss.
    assert!(
        transcript.contains("here is my key") && transcript.contains("please fix it"),
        "the surrounding text is untouched:\n{transcript}"
    );
}

// Verifies: gh #411 gap 3 - **the bundle is a copy**. The user's own session log
// is byte-identical afterwards.
//
// This is the row the brief insists on, because the tempting implementation
// scrubs in place and destroys the user's history.
#[test]
fn the_session_log_is_byte_identical_afterwards() {
    let root = scratch("copy");
    let id = session_with(&root, &format!("my key is {PLANTED}"));
    let before = on_disk(&root);
    assert!(
        before.contains(PLANTED),
        "the fixture really carries the key"
    );

    let plan =
        bug::stage(&root.join("data"), &root.join("project"), Some(&id), &root).expect("stage");
    let written = bug::write(&plan).expect("write");
    assert!(!entry(&written.path, "session.jsonl").contains(PLANTED));

    let after = on_disk(&root);
    assert_eq!(
        before, after,
        "the session log is the user's own history: the bundle is a copy, so \
         redaction must never touch it"
    );
    assert!(
        after.contains(PLANTED),
        "and the key is still there for the user"
    );
}

// --------------------------------------------------------- pattern coverage

// Verifies: gh #411 design point 1 - one row per credential family, so the table
// cannot silently lose a pattern.
#[test]
fn every_credential_family_is_scrubbed() {
    // (label, the planted value, whether it must vanish)
    let cases: &[(&str, &str)] = &[
        (
            "openai-style key",
            "sk-test-DONOTUSE-abcdefghijklmnopqrstuvwx",
        ),
        ("github token", "ghp_DONOTUSE0000000000000000000000000000"),
        (
            "slack token",
            "xoxb-DONOTUSE-000000000000-aaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        (
            "anthropic key",
            "sk-ant-DONOTUSE-0000000000000000000000000000000000",
        ),
        (
            "bearer token",
            "Bearer DONOTUSE.abcdefghijklmnopqrstuvwxyz012345",
        ),
        (
            "jwt",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJET05PVFVTRSJ9.DONOTUSEsignature",
        ),
        (
            "oauth code",
            "code=DONOTUSE-0123456789abcdef0123456789abcdef&state=xyz",
        ),
        ("google key", "AIzaDONOTUSE0000000000000000000000000"),
        ("aws key id", "AKIADONOTUSE0000000000"),
    ];
    for (label, secret) in cases {
        let root = scratch(&format!("family-{}", label.replace(' ', "-")));
        let id = session_with(&root, &format!("value {secret} end"));
        let plan = bug::stage(&root.join("data"), &root.join("project"), Some(&id), &root)
            .unwrap_or_else(|err| panic!("{label}: stage failed: {err}"));
        let written = bug::write(&plan).unwrap_or_else(|err| panic!("{label}: write: {err}"));
        let transcript = entry(&written.path, "session.jsonl");
        assert!(
            !transcript.contains(secret),
            "{label} must be scrubbed, found {secret:?} in:\n{transcript}"
        );
        assert!(
            transcript.contains("value ") && transcript.contains(" end"),
            "{label}: the surrounding text survives:\n{transcript}"
        );
    }
}

// Verifies: gh #411 design point 3 - redaction errs toward scrubbing.
//
// A long opaque token next to a credential-ish word is scrubbed even though it
// might be an innocent hash: a missed secret leaks a credential, and a false
// redaction costs a maintainer one round-trip.
#[test]
fn an_ambiguous_credential_shape_is_scrubbed_rather_than_kept() {
    let root = scratch("ambiguous");
    let id = session_with(&root, "my api_key: 0123456789abcdef0123456789abcdef");
    let plan =
        bug::stage(&root.join("data"), &root.join("project"), Some(&id), &root).expect("stage");
    let written = bug::write(&plan).expect("write");
    let transcript = entry(&written.path, "session.jsonl");
    assert!(
        !transcript.contains("0123456789abcdef0123456789abcdef"),
        "an opaque value beside `api_key` is scrubbed:\n{transcript}"
    );
}

// Verifies: gh #411 design point 2 - redaction reports what it did, by count and
// kind, and **never** the secret itself (not even truncated).
#[test]
fn the_report_counts_what_was_scrubbed_without_naming_it() {
    let root = scratch("report");
    let id = session_with(
        &root,
        &format!("key {PLANTED} and bearer Bearer DONOTUSE.tokenvalue0001"),
    );
    let plan =
        bug::stage(&root.join("data"), &root.join("project"), Some(&id), &root).expect("stage");
    assert!(
        plan.scrubbed.total() >= 2,
        "both values are counted: {:?}",
        plan.scrubbed
    );
    // The report names kinds, never values.
    let summary = plan.scrubbed.summary();
    assert!(
        summary.contains("scrubbed"),
        "the report says it scrubbed: {summary}"
    );
    assert!(
        !summary.contains(PLANTED) && !summary.contains("DONOTUSE"),
        "the report never carries a secret, not even truncated: {summary}"
    );
    assert!(
        !summary.contains("sk-test"),
        "and not a prefix of one either: {summary}"
    );
}

// Verifies: gh #411 design point 5 - gh #57's foundation is untouched: a
// credential-store decoy still never enters the bundle.
#[test]
fn the_credential_store_still_never_enters() {
    let root = scratch("decoy");
    let id = session_with(&root, "it broke");
    std::fs::write(
        root.join("data").join("credentials.toml"),
        "token = \"sk-decoy-DONOTUSE\"",
    )
    .expect("decoy");
    let plan =
        bug::stage(&root.join("data"), &root.join("project"), Some(&id), &root).expect("stage");
    let written = bug::write(&plan).expect("write");
    let raw = std::fs::read(&written.path).expect("read");
    let text = String::from_utf8_lossy(&raw);
    assert!(
        !text.contains("sk-decoy"),
        "the credential store never enters the bundle (gh #57)"
    );
}

// -------------------------------------------------------------- inspection

// Verifies: gh #411 design point 4 - the inspection lists the staged path, the
// file list with sizes, and the scrub summary, and **showing it does not require
// reading the session again**.
#[test]
fn the_inspection_lists_the_path_files_sizes_and_scrub_summary() {
    let root = scratch("inspect");
    let id = session_with(&root, &format!("key {PLANTED}"));
    let plan =
        bug::stage(&root.join("data"), &root.join("project"), Some(&id), &root).expect("stage");

    let screen = plan.inspection();
    for fact in ["report.txt", "session.jsonl", "meta.json"] {
        assert!(screen.contains(fact), "the screen lists {fact}:\n{screen}");
    }
    assert!(
        screen.contains("bytes"),
        "with sizes, so the user can judge what they are sharing:\n{screen}"
    );
    assert!(
        screen.contains("scrubbed"),
        "and what was scrubbed:\n{screen}"
    );
    assert!(
        !screen.contains(PLANTED) && !screen.contains("DONOTUSE"),
        "never the secret:\n{screen}"
    );
    // The staged path is on screen so the user knows where it is.
    assert!(
        screen.contains(&plan.staging.display().to_string()),
        "the staged path is named:\n{screen}"
    );
    // No rescan: the plan was built once and every figure comes from it.
    assert!(
        plan.reads() == 1,
        "one read of the session, not one per view"
    );
}

// Verifies: gh #411 design point 4 - cancelling removes the staging directory
// entirely. A half-written bundle left in `/tmp` is the accident this prevents.
#[test]
fn cancelling_leaves_no_staging_directory() {
    let root = scratch("cancel");
    let id = session_with(&root, &format!("key {PLANTED}"));
    let plan =
        bug::stage(&root.join("data"), &root.join("project"), Some(&id), &root).expect("stage");
    let staging = plan.staging.clone();
    assert!(staging.is_dir(), "staging exists while the user decides");

    // `discard` must remove it **eagerly**, not merely leave it to the plan's
    // `Drop`: the user asked for nothing on disk, and the backstop should never
    // be the thing that does the work.
    bug::discard(&plan);
    assert!(
        !staging.exists(),
        "cancel removes the staging directory at once: {}",
        staging.display()
    );
    // The backstop is real too: a plan dropped without either call leaves nothing.
    let dropped = bug::stage(&root.join("data"), &root.join("project"), Some(&id), &root)
        .expect("stage again");
    let path = dropped.staging.clone();
    drop(dropped);
    assert!(
        !path.exists(),
        "a dropped plan cannot leave a transcript in /tmp: {}",
        path.display()
    );
}

// Verifies: gh #411 design point 7 - a staging write failure is loud and cleans up.
#[test]
fn a_write_failure_is_loud_and_leaves_nothing_behind() {
    let root = scratch("writefail");
    let id = session_with(&root, &format!("key {PLANTED}"));
    let plan =
        bug::stage(&root.join("data"), &root.join("project"), Some(&id), &root).expect("stage");
    let staging = plan.staging.clone();
    // Make the destination unwritable so the final write cannot happen.
    let blocked = root.join("blocked");
    std::fs::write(&blocked, "not a directory").expect("fixture");
    let err =
        bug::write_to(&plan, &blocked.join("nested")).expect_err("a write into a file must fail");
    assert!(
        err.contains("bundle") || err.contains("write") || err.contains("directory"),
        "the failure says what happened: {err}"
    );
    assert!(
        !staging.exists(),
        "a failed write removes the staging directory, so no half-scrubbed bundle \
         is left behind: {}",
        staging.display()
    );
}

// Verifies: gh #411 design point 3's honest limit - a redaction *that finds
// nothing* still reports that, so the user is not told a silent lie.
#[test]
fn a_clean_session_reports_that_nothing_was_scrubbed() {
    let root = scratch("clean");
    let id = session_with(&root, "no secrets here, just a bug report");
    let plan =
        bug::stage(&root.join("data"), &root.join("project"), Some(&id), &root).expect("stage");
    assert_eq!(
        plan.scrubbed.total(),
        0,
        "nothing matched: {:?}",
        plan.scrubbed
    );
    assert!(
        plan.scrubbed.summary().contains("nothing"),
        "and it says so rather than staying silent: {}",
        plan.scrubbed.summary()
    );
}
