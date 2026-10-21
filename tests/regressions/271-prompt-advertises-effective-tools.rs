//! gh #271 (NFR-24): the system prompt advertises the tools the run offers.
//!
//! `agent_system_prompt` hardcoded `lca_core::BUILTIN_TOOLS`, so
//! `lca -p "…" --no-tools` instructed the model about eight tools the same
//! request did not declare — the prompt actively contradicting the
//! declaration, which invites wasted turns and "tool not found" confusion.
//!
//! **Structural, deliberately**, and for a stated reason: the behavioural
//! proof lives in `crates/lca-cli/tests/prompt_tools.rs`, which drives the real
//! binary and reads the prompt the provider received. That harness needs
//! `CARGO_BIN_EXE_lca`, which only the `lca-cli` test target has — this crate
//! has no binary. So the NFR-24 row pins the *shape of the fix* here: one
//! resolution, shared with the request, and no hardcoded list left in the
//! prompt.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failed assertion.

fn source(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|err| panic!("read {path}: {err}"))
}

// Verifies: gh #271 - the prompt module no longer hardcodes the built-in list;
// it takes the effective set from its caller, and exposes the one resolution
// both the prompt and the request use.
#[test]
fn the_prompt_takes_the_effective_tool_set() {
    let prompt = source("crates/lca-cli/src/prompt.rs");
    assert!(
        prompt.contains("tool_names,"),
        "the composer is fed the caller's tool names"
    );
    assert!(
        prompt.contains("pub fn effective_tools("),
        "one resolution for the prompt and the request lives here (gh #271)"
    );
    assert!(
        prompt.contains("is_builtin_active") && prompt.contains("declared_tool_specs"),
        "the resolution uses the same predicates the request's declaration uses"
    );
}

// Verifies: gh #271 - `agent_system_prompt_for` is the entry the front ends
// call, and the hardcoded list is gone from the assembly path.
#[test]
fn no_front_end_passes_a_hardcoded_list() {
    for file in [
        "crates/lca-cli/src/headless.rs",
        "crates/lca-cli/src/tui/mod.rs",
    ] {
        let src = source(file);
        assert!(
            src.contains("effective_tools("),
            "{file} resolves the effective set (gh #271)"
        );
        assert!(
            !src.contains("agent_system_prompt("),
            "{file} uses the effective-set entry point, not the built-in default"
        );
    }
}
