//! Regression rows for gh #320: `Capabilities::state_write` must publish a
//! value atomically.
//!
//! The defect: `state_write` called `std::fs::write(&path, value)`, which
//! truncates the destination before writing it. A crash, a full disk, or a
//! short write between the truncate and the last byte left the extension's
//! state file empty or half-written — and since state holds things like a
//! scroll position, a cached catalog index, or a last-seen cursor, the loss
//! is silent and permanent (the next read sees a shorter, valid-looking
//! value rather than an error).
//!
//! The sibling `write_credentials` in the same file already did
//! temp-file-then-rename; `state_write` did not.
//!
//! Verifies: gh #320 - a state value is published whole or not at all, and
//! a leftover temp file is invisible to `state_list`/`state_read`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.
use std::sync::{Arc, Mutex};

use lca_permissions::{Decision, GrantStore, PermissionPrompt, ProposalDiff, ScopeRoots};
use lca_tools::{Capabilities, CapabilityGrants};

struct Deny;

impl PermissionPrompt for Deny {
    fn ask(&mut self, _action: &lca_permissions::Action) -> Decision {
        Decision::Denied
    }
    fn review_proposals(&mut self, _diff: &ProposalDiff) -> bool {
        false
    }
}

fn engine(name: &str, root: &std::path::Path) -> Capabilities {
    Capabilities::new(
        name,
        CapabilityGrants::default(),
        ScopeRoots {
            workspace: root.join("project"),
            private: root.join("private"),
            home_config: root.join("config"),
            temp: root.join("tmp"),
            state_dir: root.join("data"),
        },
        Arc::new(Mutex::new(Deny)),
        Arc::new(Mutex::new(
            GrantStore::open(&root.join("grants.json")).expect("store"),
        )),
        root.join("project"),
        None,
    )
}

/// The durable-publish property, tested where it is *observable*.
///
/// Reading the file after a write completes proves nothing: an in-place
/// `fs::write` has also finished by then. The property that actually
/// distinguishes atomic publish from in-place truncate-and-write is that
/// the previously-published file object is never mutated - so a reader
/// holding it (an open fd, a hard link, a backup) keeps seeing the whole
/// previous value, and a crash mid-write leaves that value intact.
///
/// A hard link is the cheapest way to hold that file object: both names
/// address one inode. After an atomic replace the link still resolves to
/// the old contents; after an in-place write it shows the new ones.
#[cfg(unix)]
#[test]
fn a_concurrent_reader_of_the_previous_value_still_sees_it_whole() {
    let root = lca_testkit::scratch_path("320-durable");
    let cap = engine("alpha", &root);
    cap.state_write("cursor", b"the-complete-old-value")
        .expect("write");
    let live = cap.state_dir().join("cursor");
    let held = cap.state_dir().join("held-by-a-reader");
    std::fs::hard_link(&live, &held).expect("hard link");

    cap.state_write("cursor", b"a-brand-new-value")
        .expect("write");

    assert_eq!(
        std::fs::read(&held).expect("read the held link"),
        b"the-complete-old-value",
        "the file object a reader already holds was not mutated in place"
    );
    assert_eq!(
        std::fs::read(&live).expect("read the live path"),
        b"a-brand-new-value",
        "and the live path shows the new value"
    );
    // The held link is not a key: it lives outside the namespace's own
    // listing only because the test put it there.
    std::fs::remove_file(&held).expect("cleanup");
}

/// The published file is complete after every write. Weaker than the row
/// above, but it is the cheap invariant: no write ever leaves a short value.
#[test]
fn every_write_publishes_a_complete_value() {
    let root = lca_testkit::scratch_path("320-complete");
    let cap = engine("alpha", &root);
    for round in 0..40 {
        let value = format!("{}{}", "v".repeat(4096), round);
        cap.state_write("cursor", value.as_bytes()).expect("write");
        let read = cap.state_read("cursor").expect("read").expect("present");
        assert_eq!(
            read,
            value.as_bytes(),
            "round {round}: the published value is the whole value"
        );
    }
}

/// No temporary file may be visible through the state bag's own API. A
/// crash can leave one behind; `state_list` is what the guest enumerates,
/// so a `.tmp` sibling must never appear there.
#[test]
fn state_list_never_shows_a_temporary_file() {
    let root = lca_testkit::scratch_path("320-list");
    let cap = engine("alpha", &root);
    cap.state_write("alpha", b"a").expect("write");
    cap.state_write("beta", b"bb").expect("write");
    let listed = cap.state_list().expect("list");
    assert_eq!(
        listed,
        vec![("alpha".to_string(), 1), ("beta".to_string(), 2)],
        "only the two real keys are listed"
    );
    for (key, _) in &listed {
        assert!(
            !key.ends_with(".tmp") && !key.starts_with('.'),
            "no temp artifacts in the listing: {key:?}"
        );
    }
}

/// A temp file left behind by a crashed write must be invisible to
/// `state_read` too: reading key `alpha` never resolves to `alpha.tmp`.
#[test]
fn a_planted_stale_temp_file_is_invisible_to_read_and_list() {
    let root = lca_testkit::scratch_path("320-stale");
    let cap = engine("alpha", &root);
    cap.state_write("alpha", b"good").expect("write");
    let dir = cap.state_dir();
    // The exact artifact a crash between the temp write and the rename
    // leaves: `.<key>.tmp`. Note `alpha.tmp` is *not* an artifact - it is a
    // legal key in its own right, which is exactly why the temp name has to
    // be dot-prefixed rather than `with_extension("tmp")` (that would have
    // mapped key `alpha` onto key `alpha.tmp`'s file).
    std::fs::write(dir.join(".alpha.tmp"), b"CORRUPT").expect("plant");
    assert_eq!(
        cap.state_read("alpha").expect("read").as_deref(),
        Some(&b"good"[..]),
        "the real value is read, not the leftover"
    );
    let listed = cap.state_list().expect("list");
    assert_eq!(
        listed,
        vec![("alpha".to_string(), 4)],
        "the leftover is not a key"
    );
}

/// `alpha.tmp` is a legal key and must round-trip: the temp artifact name
/// must not collide with it. This is the row that fails if the
/// implementation ever switches to `Path::with_extension("tmp")`.
#[test]
fn a_key_named_like_a_temp_suffix_is_still_a_real_key() {
    let root = lca_testkit::scratch_path("320-dot-tmp-key");
    let cap = engine("alpha", &root);
    cap.state_write("alpha.tmp", b"real").expect("write");
    cap.state_write("alpha", b"other").expect("write");
    assert_eq!(
        cap.state_read("alpha.tmp").expect("read").as_deref(),
        Some(&b"real"[..]),
        "the dotted key keeps its own value"
    );
    assert_eq!(
        cap.state_read("alpha").expect("read").as_deref(),
        Some(&b"other"[..]),
        "and the plain key is untouched"
    );
    let listed = cap.state_list().expect("list");
    assert_eq!(
        listed,
        vec![("alpha".to_string(), 5), ("alpha.tmp".to_string(), 4)],
        "both are keys: {listed:?}"
    );
}

/// A leading dot is refused, because that space belongs to the temp files.
#[test]
fn a_key_beginning_with_a_dot_is_refused() {
    let root = lca_testkit::scratch_path("320-leading-dot");
    let cap = engine("alpha", &root);
    for key in [".hidden", ".alpha.tmp"] {
        assert!(
            matches!(
                cap.state_write(key, b"x"),
                Err(lca_protocol::CapabilityError::Invalid(_))
            ),
            "key {key:?} is refused"
        );
    }
}

/// The per-namespace size cap must not be inflated by a stale temp file.
/// Counting `.alpha.tmp` toward the total would make a namespace refill
/// impossible after one crash.
#[test]
fn a_stale_temp_file_does_not_count_toward_the_namespace_cap() {
    let root = lca_testkit::scratch_path("320-cap");
    let cap = engine("alpha", &root);
    cap.state_write("alpha", b"a").expect("write");
    let dir = cap.state_dir();
    std::fs::write(dir.join(".alpha.tmp"), vec![b'x'; 4096]).expect("plant");
    // A value that fits alongside the real key must still be accepted.
    let big = vec![b'y'; 1024];
    cap.state_write("beta", &big)
        .expect("a stale temp file must not consume the namespace budget");
    assert_eq!(
        cap.state_read("beta").expect("read").map(|v| v.len()),
        Some(1024)
    );
}

/// Two consecutive writes to the same key replace cleanly and leave no
/// artifacts: the rename path must not accumulate siblings.
#[test]
fn replacing_a_value_leaves_exactly_one_file() {
    let root = lca_testkit::scratch_path("320-replace");
    let cap = engine("alpha", &root);
    for _ in 0..5 {
        cap.state_write("key", b"same-length-value").expect("write");
    }
    let dir = cap.state_dir();
    let files: Vec<String> = std::fs::read_dir(&dir)
        .expect("read_dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        files,
        vec!["key".to_string()],
        "one file, no temp siblings: {files:?}"
    );
}

/// Deleting then writing again works: `state_delete` and the temp path must
/// not interfere.
#[test]
fn delete_then_write_round_trips() {
    let root = lca_testkit::scratch_path("320-delete");
    let cap = engine("alpha", &root);
    cap.state_write("key", b"one").expect("write");
    cap.state_delete("key").expect("delete");
    assert_eq!(cap.state_read("key").expect("read"), None);
    cap.state_write("key", b"two").expect("write");
    assert_eq!(
        cap.state_read("key").expect("read").as_deref(),
        Some(&b"two"[..])
    );
}
