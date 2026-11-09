//! Gate-11 guard for gh #574: a name moved out of `state.rs` keeps the path
//! `state.rs` gave it.
//!
//! ## Why this needs a guard rather than a compiler
//!
//! The split moved 64 public names (55 host-callback aliases, 9 prompt/modal
//! structs) into `state_hooks.rs` and `state_modals.rs`, and preserved every
//! path with a `pub use` in `state.rs` - the pattern this crate already used
//! for `setting_row.rs` (gh #30). `rustc` proves the re-exports *today* (a
//! probe against the built rlib resolves `lca_ui::state::<Name>` for all
//! sixty-four), but nothing in the tree stops the next split from moving one
//! more name and forgetting the line:
//!
//! - removing a `pub use` breaks the build only if something **inside**
//!   `lca-ui` still spells that name as `crate::state::<Name>`;
//! - a name that embedders use and this crate does not (`SwitchPrompt`,
//!   `RegionRenderer`) can vanish from `state.rs` while every in-crate file,
//!   every test here and `cargo build` stay green - and the failure lands on
//!   a downstream host at its next `cargo update`.
//!
//! That is the same class as gh #332/#410's "one fact, two owners": a public
//! name is owned by the module that promises it, and a file move must not be
//! able to take it away.
//!
//! ## What this pins
//!
//! 1. every `pub type`/`pub struct`/`pub enum` defined at the top level of
//!    `state_hooks.rs` or `state_modals.rs` appears in the `pub use` block
//!    `state.rs` keeps for that module;
//! 2. the scan actually found names (a rename that leaves it matching nothing
//!    fails here rather than passing quietly).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // test code: a panic here is a failure.

use std::path::Path;

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|err| panic!("read {rel}: {err}"))
}

const STATE: &str = "crates/lca-ui/src/state.rs";
const HOOKS: &str = "crates/lca-ui/src/state_hooks.rs";
const MODALS: &str = "crates/lca-ui/src/state_modals.rs";

/// Every name `module` defines publicly at its top level. Only item
/// declarations that start at column zero: a `pub` inside an `impl` block is a
/// method and is reached through its type, not through a module path.
fn public_definitions(module: &str, label: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in module.lines() {
        let Some(rest) = line.strip_prefix("pub ") else {
            continue;
        };
        for kind in ["type ", "struct ", "enum ", "const "] {
            let Some(after) = rest.strip_prefix(kind) else {
                continue;
            };
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                names.push(name);
            }
        }
    }
    assert!(
        !names.is_empty(),
        "{label} defines no public item; the guard would pass by inspecting nothing"
    );
    names
}

/// The names listed in a `pub use <path>::{ … };` block, or a panic naming
/// the block that went missing.
///
/// The block is matched by **counting braces**, not by `split("};")`. The
/// split version was the first implementation and it was wrong in a way that
/// made the guard vacuous: `state.rs`'s next item after the alias block also
/// ends in `};`, so a name missing from the alias block could still be found
/// past the boundary. Counting is what makes a truncated block fail.
fn reexported(state: &str, path: &str) -> Vec<String> {
    let header = format!("pub use {path}::{{");
    let start = state.find(&header).unwrap_or_else(|| {
        panic!("{STATE} has no `{header} …}};` block; a moved name cannot be reached")
    });
    let rest = &state[start + header.len()..];
    let mut depth = 1usize;
    let mut end = None;
    for (at, c) in rest.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(at);
                    break;
                }
            }
            _ => {}
        }
    }
    let block =
        &rest[..end.unwrap_or_else(|| panic!("{STATE}'s `{header}` block is unterminated"))];
    block
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|token| !token.is_empty())
        .map(|token| token.trim_end_matches(',').to_string())
        .collect()
}

/// Every public item `module_path` defines must be listed in the
/// `pub use {reexport_path}` block, or the guard fails naming the missing ones.
fn each_definition_is_reexported(module_path: &str, reexport_path: &str, label: &str) {
    let state = read(STATE);
    let names = public_definitions(&read(module_path), label);
    let listed = reexported(&state, reexport_path);
    let mut missing = Vec::new();
    for name in &names {
        if !listed.iter().any(|token| token == name) {
            missing.push(name.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "{}: {STATE} does not re-export {missing:?} from {reexport_path}, so the name changed its \
         public path. Every name moved into {label} must be listed in that `pub use` block - \
         in-crate callers may not notice, a host's next `cargo update` will",
        names.len()
    );
}

// Verifies: gh #574 - a name moved out of `state.rs` keeps the path `state.rs`
// promised, so a split can never silently rename a host hook.
#[test]
fn every_moved_state_name_keeps_its_reexport() {
    each_definition_is_reexported(HOOKS, "crate::state_hooks", HOOKS);
    each_definition_is_reexported(MODALS, "crate::state_modals", MODALS);
}

// Verifies: gh #574 - the crate root still names the moved structs it named
// before the split, so a host that writes `use lca_ui::DialogModal` is not
// pushed onto `lca_ui::state::DialogModal` by a file move.
#[test]
fn the_crate_root_still_names_the_moved_prompts() {
    let lib = read("crates/lca-ui/src/lib.rs");
    let block = reexported(&lib, "state");
    // The moved names the crate root already promised before the split. This
    // list is deliberately the pre-split content, not "every moved name":
    // `PermissionModal` and `SecretPrompt` were never at `lca_ui::<Name>`, and a
    // guard that demanded them would be inventing a surface the crate never had
    // - the first version of this row did exactly that and failed on its own
    // expectation, which is how it was found.
    for name in ["DialogExchange", "DialogModal", "PromptRequest"] {
        assert!(
            block.iter().any(|token| token == name),
            "`lca_ui::{name}` no longer resolves: lib.rs's `pub use state::{{…}}` block does not \
             list it"
        );
    }
}
