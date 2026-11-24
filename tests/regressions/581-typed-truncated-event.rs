//! Gh #581 — the truncation crossed the guest boundary as a stringly-typed vendor
//! event, and now it is a typed case. This is the row that says the old shape is
//! **gone from every guest**, not from the three files the issue happened to name.
//!
//! # Why a source scan
//!
//! The defect class the typed case removes is a *spelling* defect: `vendor-event
//! ("truncated", "")` is checked on both sides by a shared constant, so the two
//! halves cannot disagree at run time - but a typo in either side's spelling is a
//! compile-time no-op and a runtime loss. A variant case makes the compiler carry
//! it, which is only true while no guest still assembles the string. The behavioural
//! rows (`crates/lca-ext-host/tests/provider/parity.rs`) prove the round trip works;
//! this proves nobody left the escape hatch open behind them.
//!
//! # The scan reads the tree, not a filename list
//!
//! Gh #574's rule: a guard that greps one filename stops seeing the code a split
//! moves. A **directory** walk is the honest shape here - the class is "any guest
//! source", and the guest set is `extensions/*/src` plus the shared kit. A provider
//! added tomorrow is covered by being in that tree; one that copies the old shape
//! fails this row without anybody remembering to list it.
//!
//! # The compat read is deliberate, and it is the one consumer
//!
//! `crates/lca-host`'s stream mapping still reads `vendor-event("truncated", "")`,
//! because a component built against `@0.7.0` before the case landed can still send
//! it. That is why `TRUNCATED_KIND` still exists at all, and why this row asserts
//! both halves: **no guest names the kind**, and **the host still reads it**. When
//! the read is finally removed, this row keeps the guest half and the host assertion
//! changes with it - the removal condition is named beside the read itself.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The guest sources: every extension crate's own code, plus the shared subscription
/// kit (which is the guest of `codex` and `grok`). Tests inside those crates are
/// included on purpose - a stale `#[cfg(test)]` copy of the shim is still a copy
/// somebody will paste.
fn guest_sources() -> Vec<(String, String)> {
    let root = root();
    let mut files = Vec::new();
    let extensions = root.join("extensions");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&extensions)
        .expect("extensions/")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        let src = dir.join("src");
        if src.is_dir() {
            files.extend(rust_files(&src));
        }
    }
    files.extend(rust_files(&root.join("crates/lca-subscription/src")));
    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            (rel, text)
        })
        .collect()
}

fn code_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|line| {
        let trimmed = line.trim_start();
        !trimmed.starts_with("//") && !trimmed.starts_with("/*") && !trimmed.starts_with('*')
    })
}

// Verifies: gh #581 (§3, the structural row) — **no in-tree guest still names the
// vendor-event kind.** The typed case exists so that a guest cannot get the
// truncation's spelling wrong; a guest that still assembles the kind string has a
// door open that the compiler no longer watches. This is the check that makes the
// compat read's removal condition ("no in-tree guest emits it") verifiable rather
// than aspirational.
#[test]
fn no_in_tree_guest_smuggles_the_cut_through_a_vendor_event() {
    // **One deliberate exception**: the conformance fixture's scripts *play* a
    // pre-typed guest, so the compat read has a producer to read. That is what
    // `provider/parity.rs`'s `the_pre_typed_vendor_event_still_maps_to_the_typed_fact`
    // and `a_guest_reporting_the_cut_twice_delivers_it_once` drive. Any *other* file
    // naming the kind is a guest that kept the door open.
    let fixture_scripts = "extensions/conformance/src/scripts.rs";
    let mut offenders = Vec::new();
    let mut simulated = 0;
    for (path, text) in guest_sources() {
        for (number, line) in code_lines(&text).enumerate() {
            if !line.contains("TRUNCATED_KIND") {
                continue;
            }
            if path == fixture_scripts {
                simulated += 1;
            } else {
                offenders.push(format!("{path}:{}: {}", number + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a guest still re-encodes the truncation as a vendor event; it has a typed case \
         since the 0.7 line, so emit `StreamEvent::Truncated` instead (gh #581):\n{}",
        offenders.join("\n")
    );
    assert_eq!(
        simulated, 2,
        "the fixture still simulates both pre-typed shapes (one vendor event, and one \
         stream carrying both) - if these went away the compat read lost its producer"
    );
}

// Verifies: gh #581 (§3, the structural row) — **every guest that maps the protocol
// stream forwards the typed case.** Finding the mappers by their `error` arm rather
// than by filename means a provider added later cannot be born with the shim: the
// file that translates events has one way to translate this one, or this row names
// it. The `error` arm is the marker because every mapper must have it (an error is
// not optional in the stream), while `truncated` is the arm under test.
#[test]
fn every_guest_that_maps_the_stream_forwards_the_typed_case() {
    let mut mappers = 0;
    let mut missing = Vec::new();
    for (path, text) in guest_sources() {
        let code = code_lines(&text).collect::<Vec<_>>().join("\n");
        if !code.contains("P::Error { message, retryable } => WasmEvent::Error") {
            continue;
        }
        mappers += 1;
        if !code.contains("P::Truncated => WasmEvent::Truncated") {
            missing.push(path);
        }
    }
    // The two shared kits (subscription, conformance is a fixture not a kit) plus the
    // eight provider copies. A lower number means the marker changed and the row went
    // vacuous, which is the failure mode a scan like this has.
    assert!(
        mappers >= 10,
        "only {mappers} stream mappers found; the marker ({}) stopped matching, so this \
         row proves nothing",
        "P::Error { message, retryable } => WasmEvent::Error"
    );
    assert!(
        missing.is_empty(),
        "these guests map the stream but do not forward the typed truncation (gh #581): \
         {missing:?}"
    );
}

// Verifies: gh #581 (§2.2) — **the case is on the wire variant.** The whole change is
// this one line; a `wit/` edit that missed it, or a later edit that dropped it while
// the Rust side kept compiling against a cached binding, would leave the typed path
// fictional. Read as text because the WIT is not compiled by `cargo check` for the
// native target.
#[test]
fn the_stream_variant_carries_the_typed_case() {
    let wit = std::fs::read_to_string(root().join("wit/world-provider.wit"))
        .expect("wit/world-provider.wit");
    let start = wit
        .find("variant stream-event {")
        .expect("the provider world still declares `stream-event`");
    let block = &wit[start..];
    let end = block.find("\n  }").expect("the variant closes");
    let variant = &block[..end];
    assert!(
        variant.lines().any(|line| line.trim() == "truncated,"),
        "the case is payload-free and named `truncated`:\n{variant}"
    );
    // No payload: the host's banner is the only consumer, and
    // `lca_protocol::StreamEvent::Truncated` is a unit variant. A tuple or record here
    // would invent information the guest does not have.
    for line in variant.lines().filter(|line| line.contains("truncated")) {
        let trimmed = line.trim();
        assert!(
            trimmed == "truncated," || trimmed.starts_with("///"),
            "the truncation case carries no payload: {trimmed}"
        );
    }
}

// Verifies: gh #581 (§2.4) — **the host keeps the compat read, and prefers the typed
// case.** Both halves matter: without the read, a pre-typed guest's cut answer arrives
// as a complete one (silently); without the typed arm, a rebuilt guest's cut would have
// to ride the vendor event it was supposed to stop needing. The host module is walked,
// not named (gh #552 moved this mapping once already).
#[test]
fn the_host_reads_both_shapes_and_prefers_the_typed_one() {
    let host = crate::module_text("crates/lca-ext-host/src/host_imports.rs");
    let code = code_lines(&host).collect::<Vec<_>>().join("\n");
    assert!(
        code.contains("StreamEvent::Truncated => P::Truncated"),
        "the typed case maps to the real variant"
    );
    assert!(
        code.contains("if kind == lca_protocol::TRUNCATED_KIND"),
        "the pre-typed vendor event still maps, for one minor version"
    );
    let typed = code
        .find("StreamEvent::Truncated => P::Truncated")
        .expect("checked above");
    let compat = code
        .find("if kind == lca_protocol::TRUNCATED_KIND")
        .expect("checked above");
    assert!(
        typed < compat,
        "the typed arm is written first, so the compat read is visibly the fallback \
         rather than the path"
    );
    // The dedupe lives where events cross into the core, so a guest that reports the
    // cut twice still delivers one event (the row in `provider/parity.rs` drives it).
    let provider = crate::module_text("crates/lca-ext-host/src/provider.rs");
    assert!(
        code_lines(&provider)
            .collect::<Vec<_>>()
            .join("\n")
            .contains("delivered_truncated"),
        "the stream loop delivers the fact once"
    );
}
