//! Gh #352 — a thinking run's replay signature crossed the guest boundary as an
//! extras-map convention, and now it is a typed `content-block` case. This row says
//! the convention is **gone from every guest**, not from the one the issue named.
//!
//! # Why a source scan
//!
//! The defect class the typed case removes is a *convention* defect: the host wrote
//! `thinking-signature` into a message's reserved extras and every signature-needing
//! guest rebuilt a `Reasoning` block from it. Both halves were spelled by hand, so a
//! typo on either side was a compile-time no-op and a runtime loss (the provider then
//! answers HTTP 400 because it replayed a call without its signature). A variant case
//! makes the compiler carry the fact - which is only true while no guest still
//! assembles the key.
//!
//! # The scan reads the tree, not a filename list
//!
//! Gh #574's rule: a guard that greps one filename stops seeing the code a split
//! moves. The class is "any guest source", and the guest set is `extensions/*/src`
//! plus the shared subscription kit - a directory walk, so a provider added tomorrow
//! is covered by being in that tree (the #581 row is the same shape, and its own
//! Windows bug taught this one to normalise separators).
//!
//! # The compat write is deliberate, and it is the one writer
//!
//! The host still **writes** the key (`to_wit_request`), because a guest built against
//! `@0.7.0` before the case reads it and would otherwise lose the signature. That is
//! why the constant still exists at all, and why this row asserts both halves: **no
//! guest names the key**, and **the host still writes it**. The removal condition is
//! named where the write lives.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

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

/// Every guest source: each extension crate's own code plus the shared subscription
/// kit (the guest of `codex` and `grok`), with paths normalised to `/` so the same row
/// behaves on every platform.
fn guest_sources() -> Vec<(String, String)> {
    let root = root();
    let mut files = Vec::new();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root.join("extensions"))
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
                .to_string()
                .replace('\\', "/");
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

// Verifies: gh #352 acceptance criterion 1 (the structural half) — **no in-tree guest
// reads the extras key.** The typed case exists so a guest cannot get the signature's
// channel wrong; a guest that still looks in the reserved map has a door the compiler
// does not watch. A guest *cannot* use a key it never names, so this is the proof the
// behavioural rows lean on.
#[test]
fn no_in_tree_guest_reads_the_thinking_signature_extra() {
    let mut offenders = Vec::new();
    for (path, text) in guest_sources() {
        for (number, line) in code_lines(&text).enumerate() {
            if line.contains("THINKING_SIGNATURE_EXTRA") || line.contains("thinking-signature") {
                offenders.push(format!("{path}:{}: {}", number + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a guest still rebuilds a reasoning block from the reserved extras key; the 0.7 \
         line has a typed `reasoning` case, so read `ContentBlock::Reasoning` instead \
         (gh #352):\n{}",
        offenders.join("\n")
    );
}

// Verifies: gh #352 — **every guest that maps a content block maps the reasoning
// case.** Found by the `image` arm rather than by filename (every content-block
// mapper must have one, a reasoning arm is optional only in the sense that the
// compiler will not ask for it if the match ends in `_`), so a provider added later
// cannot be born dropping the block silently.
#[test]
fn every_content_block_mapper_carries_the_reasoning_case() {
    let mut mappers = 0;
    let mut missing = Vec::new();
    for (path, text) in guest_sources() {
        let joined = code_lines(&text).collect::<Vec<_>>().join("\n");
        // The protocol -> WIT direction names the WIT type; the WIT -> protocol
        // direction names the protocol one. A mapper has at least one of them.
        // An arm, not a detection: `matches!(block, ContentBlock::Image { .. })`
        // is how a provider asks "does this request carry an image?", and it has
        // no business gaining a reasoning arm.
        let maps_protocol =
            joined.contains("lca_protocol::ContentBlock::Image { media_type, bytes } =>");
        let maps_wit = joined.contains("ContentBlock::Image((media_type, bytes)) =>");
        if !maps_protocol && !maps_wit {
            continue;
        }
        mappers += 1;
        if !joined.contains("ContentBlock::Reasoning") {
            missing.push(path);
        }
    }
    // The host's two bridges, the kit, and eleven guests carried the arm when this
    // row was written. A number that drops means the marker stopped matching and the
    // row went vacuous, which is the failure mode a scan like this has.
    assert!(
        mappers >= 13,
        "only {mappers} content-block mappers found; the marker stopped matching, so \
         this row proves nothing"
    );
    assert!(
        missing.is_empty(),
        "these guests map content blocks but drop the reasoning block (gh #352): \
         {missing:?}"
    );
}

// Verifies: gh #352 — **the wire variant carries the case, and it carries both
// fields.** `text` is what makes full-fidelity replay possible (the doc rule is to
// return the whole response); `signature` is what a provider requires echoed. A WIT
// edit that shipped only one would pass every row that checks "the case exists".
#[test]
fn the_content_block_variant_carries_text_and_an_optional_signature() {
    let wit = std::fs::read_to_string(root().join("wit/types.wit")).expect("wit/types.wit");
    let record = wit
        .find("record reasoning-block {")
        .expect("the reasoning-block record exists");
    let record_text = &wit[record..];
    let end = record_text.find("\n  }").expect("the record closes");
    let record_text = &record_text[..end];
    assert!(
        record_text.contains("text: string,"),
        "the record carries the reasoning text:\n{record_text}"
    );
    assert!(
        record_text.contains("signature: option<string>,"),
        "the signature is optional, because absent and empty are different facts \
         (a provider rejects an empty one):\n{record_text}"
    );
    let variant = wit
        .find("variant content-block {")
        .expect("the content-block variant exists");
    let variant_text = &wit[variant..];
    let end = variant_text.find("\n  }").expect("the variant closes");
    assert!(
        variant_text[..end]
            .lines()
            .any(|line| line.trim() == "reasoning(reasoning-block),"),
        "the variant carries the case:\n{}",
        &variant_text[..end]
    );
}

// Verifies: gh #352 acceptance criterion 1 (the compat half) — **the host still
// writes the key.** A guest built before the case reads it, and dropping the write
// would lose that guest's signature silently - the exact defect the typed case
// removes for converted guests. The write is what the removal condition in the
// comment waits on, so both halves are asserted together.
#[test]
fn the_host_still_writes_the_compat_key_for_pre_0_7_guests() {
    let host = crate::module_text("crates/lca-ext-host/src/host_imports/wit.rs");
    let code = code_lines(&host).collect::<Vec<_>>().join("\n");
    assert!(
        code.contains("ContentBlock::Reasoning"),
        "the typed case is mapped"
    );
    assert!(
        code.contains("THINKING_SIGNATURE_EXTRA.to_string()"),
        "and the pre-0.7 extras key is still written beside it"
    );
    let typed = code.find("ContentBlock::Reasoning").expect("checked above");
    let compat = code
        .find("THINKING_SIGNATURE_EXTRA.to_string()")
        .expect("checked above");
    assert!(
        typed < compat,
        "the typed case is written first, so the compat channel reads as the fallback \
         rather than the path"
    );
}
