//! GitHub #377: a binding on a shifted symbol (`?`, `ctrl+?`, `alt+~`) can be written
//! down naturally and matches.
//!
//! Kitty flag 4 reports `Ctrl+Shift+/` as `\x1b[47:63;6u` — codepoint `'/'` (47) plus a
//! **separate shifted key** `'?'` (63) with `mod=ctrl|shift`. `kitty.rs` parsed those
//! shifted digits and then **threw them away**, so a binding on `ctrl+?` compared 63
//! against 47 and missed. The user had to discover the physical spelling
//! `ctrl+shift+/` by accident, and a binding that cannot be written down naturally is a
//! keyboard the user cannot use.
//!
//! The behavioural rows live in `crates/lca-tui/src/engine/keys_tests.rs`. This file is
//! the NFR-24 named guard: it pins the structure that makes the fix hold — the shifted
//! key is *kept*, the vocabulary is *bounded*, both protocols reach *one* decision, and
//! the physical spelling is not deprecated.

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path:?}: {err}"))
}

fn keys_source() -> String {
    let source = read("crates/lca-tui/src/engine/keys.rs");
    match source.rfind("\n#[cfg(test)]\n#[path") {
        Some(at) => source[..at].to_string(),
        None => source,
    }
}

fn kitty_source() -> String {
    let source = read("crates/lca-tui/src/engine/keys/kitty.rs");
    match source.rfind("\n#[cfg(test)]") {
        Some(at) => source[..at].to_string(),
        None => source,
    }
}

fn function_body<'a>(source: &'a str, signature: &str, closer: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} exists"));
    let rest = &source[start..];
    let end = rest.find(closer).map_or(rest.len(), |at| at + closer.len());
    &rest[..end]
}

// Verifies: gh #377 rule 1 - **the shifted key is kept**. The fix's precondition is that
// `ParsedKitty` carries the digits the protocol already sends.
//
// The pre-fix code had a comment saying "optional digits for shifted key; skip". If the
// field ever goes away, the logical path has nothing to compare and the whole fix
// silently reverts to "misses".
#[test]
fn the_kitty_parser_keeps_the_shifted_key() {
    let source = kitty_source();
    assert!(
        source.contains("shifted_key: Option<u32>"),
        "ParsedKitty must carry the shifted key"
    );
    assert!(
        !source.contains("shifted key; skip"),
        "the pre-fix comment marks the exact place the data was discarded"
    );
    // Every construction must populate it, or one protocol shape would silently lack it.
    let constructions = source.matches("ParsedKitty {").count();
    let populated = source.matches("shifted_key:").count();
    assert!(
        populated >= constructions,
        "{constructions} ParsedKitty constructions but only {populated} set shifted_key"
    );
}

// Verifies: gh #377 rule 2 - the logical candidate **consumes only Shift**.
//
// Dropping other modifiers would trade one wrong match for another: `ctrl+alt+?` would
// fire on a bare `ctrl+?`. The subtraction is the contract.
#[test]
fn the_logical_candidate_consumes_only_shift() {
    let source = keys_source();
    let body = function_body(&source, "fn logical_shifted_symbol(", "\n}\n");
    assert!(
        body.contains("modifier & !MOD_SHIFT"),
        "only Shift is removed from the modifier set: {body}"
    );
    for other in ["MOD_ALT", "MOD_CTRL", "MOD_SUPER"] {
        assert!(
            !body.contains(&format!("& !{other}")),
            "the candidate must not consume {other}: {body}"
        );
    }
}

// Verifies: gh #377 rule 3 - the **symbol vocabulary is bounded**, and it lives in one
// place.
//
// An unbounded candidate would make a shifted letter collapse onto its unshifted
// binding (`ctrl+shift+a` firing `ctrl+a`), which the physical path would not do - two
// paths disagreeing about one keystroke. The set is pi's `SHIFTED_SYMBOL_KEYS`.
#[test]
fn the_symbol_vocabulary_is_bounded_and_lives_in_one_place() {
    let source = keys_source();
    assert!(
        source.contains("const SHIFTED_SYMBOL_KEYS: &str"),
        "the shifted-symbol vocabulary must be a named constant"
    );
    let body = function_body(&source, "fn logical_shifted_symbol(", "\n}\n");
    assert!(
        body.contains("SHIFTED_SYMBOL_KEYS.contains(ch)"),
        "the candidate must consult the bounded set: {body}"
    );
    // Exactly one definition, so it cannot drift into a second copy.
    assert_eq!(
        source.matches("SHIFTED_SYMBOL_KEYS: &str =").count(),
        1,
        "one definition"
    );
    // And the set is pi's, character for character.
    let line = source
        .lines()
        .find(|l| l.contains("SHIFTED_SYMBOL_KEYS: &str ="))
        .expect("the constant");
    assert!(
        line.contains(r#""~!@#$%^&*()_+|{}:<>?""#),
        "the vocabulary is pi's SHIFTED_SYMBOL_KEYS: {line}"
    );
}

// Verifies: gh #377 rule 4 - **one matcher, two protocols**.
//
// Kitty and `modifyOtherKeys` must reach the same logical decision through the same
// function. A second shifted-symbol rule in either file is the divergence this repo has
// closed four times in two days.
#[test]
fn both_protocols_call_the_same_logical_rule() {
    let kitty = kitty_source();
    assert!(
        kitty.contains("matches_logical_shifted_symbol("),
        "kitty must call the shared rule"
    );
    assert!(
        !kitty.contains("SHIFTED_SYMBOL_KEYS"),
        "kitty must not carry its own vocabulary"
    );
    let keys = keys_source();
    // The modifyOtherKeys path calls it too.
    let body = function_body(&keys, "fn matches_printable_modify_other_keys(", "\n}\n");
    assert!(
        body.contains("matches_logical_shifted_symbol("),
        "modifyOtherKeys must call the same rule: {body}"
    );
    // The rule itself is defined once.
    assert_eq!(
        keys.matches("fn matches_logical_shifted_symbol(").count(),
        1,
        "exactly one logical-rule implementation"
    );
    assert_eq!(
        kitty.matches("fn matches_logical_shifted_symbol(").count(),
        0,
        "and kitty does not define a second one"
    );
}

// Verifies: gh #377 rule 5 - **the physical spelling is not deprecated**.
//
// `ctrl+shift+/` is what users had to discover by accident; adding the natural spelling
// must not remove it. The physical branch must still be reachable *before* the modifier
// short-circuit, which is the ordering bug I hit: the early `if actual_mod != expected_mod
// { return false; }` ran before the logical branch and made the whole fix dead code.
#[test]
fn the_physical_branch_precedes_the_modifier_short_circuit() {
    let kitty = kitty_source();
    let body = function_body(&kitty, "pub(super) fn matches_kitty(", "\n}\n");
    let physical = body
        .find("actual_mod == expected_mod && normalized == normalized_expected")
        .expect("the physical branch compares codepoint and modifiers together");
    let logical = body
        .find("matches_logical_shifted_symbol(")
        .expect("the logical branch exists");
    let short_circuit = body
        .find("if actual_mod != expected_mod {")
        .expect("the blanket modifier rejection still guards the base-layout branch");
    assert!(
        physical < logical && logical < short_circuit,
        "physical first, then logical, then the blanket rejection: {body}"
    );
}
