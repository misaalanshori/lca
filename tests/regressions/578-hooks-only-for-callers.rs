//! Gh #578: an extension must not implement a hook the host can never deliver.
//!
//! `on-session-close` is exported by exactly one world, `hooks`
//! (`wit/world-hooks.wit`). The host dispatches every one of the six hook points
//! only to a handle whose `worlds()` declares `Hooks` - `ExtensionRegistry`'s
//! per-hook filters, all five of them plus `on_session_close` - so a body written
//! by a handle that declares any other world is **unreachable**.
//!
//! That is not a mistake the type system used to prevent. `ExtensionDispatch` is
//! one flat trait covering every world, and until this cycle its six hook methods
//! were **required**, so the compiler forced every implementor to write them. The
//! reference `ui` extension complied and shipped a dead `on_session_close` under a
//! comment explaining that it was dead. Nobody was lying; the trait was.
//!
//! Two halves of the fix, and this file guards both:
//!
//! 1. **The trait** - the six methods have default bodies now, so a world that
//!    does not export them inherits an observation no-op instead of an
//!    obligation. Guarded by [`the_six_hook_methods_are_defaulted`].
//! 2. **The guests** - no native implementor may override a hook method without
//!    declaring `hooks`. Guarded by
//!    [`no_native_extension_implements_a_hook_it_cannot_receive`], which reads the
//!    source because that is the only place the two facts sit side by side.
//!
//! # Why a source guard is the right instrument for the native half
//!
//! `DeliveryMode::Native` means **compiled into the binary**: there is no
//! dynamic loading path (`grep libloading` finds nothing), so every native
//! implementor in existence is a file in this repository. A source-derived row
//! therefore covers the whole population, which a runtime check could not - a
//! native handle has no export table to compare a manifest against.
//!
//! The WASM half *is* checkable from the component's own type: a component that
//! exports `hook-session-close` while its manifest declares `worlds = ["ui"]` is
//! self-contradicting. That is what
//! [`the_export_fragment_table_matches_the_wit_worlds`] and
//! [`a_component_exporting_a_hook_world_is_detected_from_its_exports`] pin.
//!
//! **The host does not act on it, and that is deliberate.** The first version of
//! this cycle made the loader *refuse* such a component, and it broke four
//! `lca-ext-host` rows: a component may legitimately export more worlds than its
//! manifest enables, because a manifest decides what is *served* and a bundle may
//! carry more than a given install uses - the conformance fixture is exactly that
//! shape, one component for every world. Refusing would have outlawed a supported
//! packaging. So the attribution predicate stays a **test instrument**: it proves
//! the mapping is right and that the contradiction is detectable, without making
//! the host police it.

use std::path::{Path, PathBuf};

/// The repository root (this crate lives at `tests/regressions/`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Read a file from the repository root.
fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found
}

/// The six `hooks`-world methods on `ExtensionDispatch`, in trait order.
const HOOK_METHODS: &[&str] = &[
    "on_pre_turn",
    "on_pre_tool_use",
    "on_post_tool_use",
    "on_post_turn_end",
    "on_attention_required",
    "on_session_close",
];

/// The `World` variant that legitimately serves the six hook points.
///
/// Only `Hooks` does - and it is spelled as the variant because that is what
/// `worlds()` answers. `HooksMessage`, `HooksToolCall` and the rest are different
/// worlds with their own methods (`on_message_end`, `on_tool_call`), which the
/// trait already defaults and which the host dispatches on their own filters, so
/// they are correctly out of scope here.
const WORLD_SERVING_HOOKS: &str = "Hooks";

/// Files that carry the old shape and are **out of this issue's lane**, with why.
///
/// Each entry is a debt, not an exemption: the list is asserted to be *small* and
/// each name is explained, so a future cycle clears it rather than growing it.
const KNOWN_DEAD_HOOKS: &[(&str, &str)] = &[(
    "crates/lca-cli/src/tui/model_select.rs",
    "A `Provider`-only test double inside `lca-cli`, which this cycle's file \
     boundary excludes. Its six hook bodies are boilerplate no-ops, so leaving \
     them is cosmetic; removing them is a one-line follow-up for whoever next \
     touches that module.",
)];

/// The worlds a handle answers from `worlds()`.
///
/// **`worlds()` is the authority for a native handle, not the manifest.** A
/// native handle wires worlds the sandbox cannot feed, and its own source says so
/// (`stream-guard`: "the manifest is the WASM install contract; native wiring is
/// not manifest-bound"), so reading `extension.toml` here would report a correct
/// extension as broken. That mistake was in this guard's first draft and the row
/// caught it - `stream-guard` legitimately answers `Hooks` for a manifest that
/// declares only `tool`.
fn declared_worlds(src: &str) -> Vec<String> {
    let Some(start) = src.find("fn worlds(&self) -> Vec<World>") else {
        return Vec::new();
    };
    let Some(open) = src[start..].find('{').map(|at| start + at) else {
        return Vec::new();
    };
    let mut depth = 0usize;
    let mut end = src.len();
    for (offset, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &src[open..end];
    let mut worlds: Vec<String> = body
        .match_indices("World::")
        .filter_map(|(at, _)| {
            let rest = &body[at + "World::".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect();
    worlds.sort();
    worlds.dedup();
    worlds
}

/// The `fn <name>` names an `impl ExtensionDispatch for … { … }` block defines.
///
/// Scans from the impl header to its matching closing brace, so a method that
/// belongs to a *different* impl (the `Drop`, the inherent methods) is not
/// attributed to this one. Brace counting is sufficient: this file's own `fmt`
/// output is what it reads, and a brace inside a string literal in a hook body
/// would be a surprising thing to write.
fn hook_methods_in_impl(src: &str) -> Vec<String> {
    let Some(header) = src.find("impl ExtensionDispatch for ") else {
        return Vec::new();
    };
    let Some(open) = src[header..].find('{').map(|at| header + at) else {
        return Vec::new();
    };
    let mut depth = 0usize;
    let mut end = src.len();
    for (offset, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &src[open..end];
    HOOK_METHODS
        .iter()
        .filter(|method| {
            // `fn on_…(` with nothing between: this catches a definition, not a
            // mention in a doc comment (those write `on_session_close` bare or in
            // backticks, without the `fn ` keyword).
            body.contains(&format!("fn {method}("))
        })
        .map(|method| (*method).to_string())
        .collect()
}

// Verifies: gh #578's first half - the trait no longer forces a dead method.
//
// While these six were required, every implementor had to write all of them, and
// a body that can never be dispatched is indistinguishable from a live one at the
// definition site. Defaulted, an undeclared world inherits the no-op the dispatch
// filter already meant, and an author who wants a hook declares `hooks`.
#[test]
fn the_six_hook_methods_are_defaulted() {
    let src = read("crates/lca-ext-abi/src/lib.rs");
    for method in HOOK_METHODS {
        let decl = format!("fn {method}");
        let at = src
            .find(&decl)
            .unwrap_or_else(|| panic!("`{method}` is declared on the trait"));
        // Slice to the end of this declaration: the first `;` or `{` after it
        // says whether a body follows.
        let rest = &src[at..];
        let semi = rest.find(';');
        let brace = rest.find('{');
        let defaulted = match (semi, brace) {
            (Some(s), Some(b)) => b < s,
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (None, None) => panic!("`{method}` has neither a `;` nor a `{{`"),
        };
        assert!(
            defaulted,
            "`{method}` must have a default body (gh #578): a required hook method \
             forces every implementor to write one the host may never call"
        );
    }
}

// Verifies: gh #578's second half, over the whole native population.
//
// `DeliveryMode::Native` means compiled in, so this row sees every implementor
// that exists. A `ui`-only handle that defines `on_session_close` is the exact
// shape that shipped last cycle, and it fails here naming the file and method.
#[test]
fn no_native_extension_implements_a_hook_it_cannot_receive() {
    let root = repo_root();
    let mut offenders: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for dir in ["extensions", "crates"] {
        for file in rust_files(&root.join(dir)) {
            if file
                .components()
                .any(|part| part.as_os_str() == "tests" || part.as_os_str() == "target")
            {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&file) else {
                continue;
            };
            if !src.contains("impl ExtensionDispatch for ") {
                continue;
            }
            let methods = hook_methods_in_impl(&src);
            if methods.is_empty() {
                continue;
            }
            checked += 1;
            let worlds = declared_worlds(&src);
            let serves_hooks = worlds.iter().any(|world| world == WORLD_SERVING_HOOKS);
            let relative = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .display()
                .to_string();
            if !serves_hooks && !KNOWN_DEAD_HOOKS.iter().any(|(path, _)| *path == relative) {
                offenders.push(format!(
                    "{}: implements {} but declares worlds = {worlds:?}",
                    relative,
                    methods.join(", ")
                ));
            }
        }
    }
    assert!(
        checked > 0,
        "the scan found no implementor at all, so it proves nothing"
    );
    assert!(
        offenders.is_empty(),
        "gh #578: these implement a hook the host can never dispatch:\n  {}",
        offenders.join("\n  ")
    );
}

// Verifies: the exemption list stays a debt list.
//
// A guard that exempts by name can rot into a place to hide things. Two
// properties keep it honest: every entry must still exist (so a fixed file is
// removed from the list rather than lingering as a lie), and the list stays
// short enough to read. The number is a ceiling, not a target.
#[test]
fn the_known_dead_hooks_list_is_small_and_current() {
    assert!(
        KNOWN_DEAD_HOOKS.len() <= 2,
        "the exemption list is a debt list, not a permanent home: {:?}",
        KNOWN_DEAD_HOOKS.iter().map(|(p, _)| *p).collect::<Vec<_>>()
    );
    for (path, reason) in KNOWN_DEAD_HOOKS {
        assert!(
            repo_root().join(path).is_file(),
            "`{path}` no longer exists, so remove it from KNOWN_DEAD_HOOKS"
        );
        assert!(
            !reason.trim().is_empty(),
            "`{path}` must say why it is exempt"
        );
    }
}

// Verifies: the reference extension - the one the issue was filed against - has
// no hook bodies at all, and says in its source why that is correct.
//
// A `worlds = ["ui"]` manifest with an `on_session_close` body is the defect in
// one line, so this row names the same file twice: once for the absence and once
// for the reason being written down next to it (NFR-24's "a comment states the
// rule", the same shape `#333`'s guard uses for its exempt dialogs).
#[test]
fn the_reference_ui_extension_implements_no_hook() {
    let src = read("extensions/ui-example/src/lib.rs");
    let methods = hook_methods_in_impl(&src);
    assert!(
        methods.is_empty(),
        "the ui extension declares only `ui`, so it must implement no hook; found {methods:?}"
    );
    assert!(
        src.contains("gh #578"),
        "and the source states why the absence is correct, so the next author does \
         not add one back"
    );
}

/// The `lca:ext` export interface names each world contributes.
///
/// The inverse of `lca_ext_abi::WORLDS`, and it lives in this file rather than the
/// ABI crate because **only a test needs it**: the host refuses nothing on the
/// strength of it (a component may legitimately bundle more worlds than its
/// manifest enables - the conformance fixture is exactly that, and a refusal broke
/// 4 of `lca-ext-host`'s rows when this was tried). A predicate with one test
/// caller and a wrong-answer cost of "reports a correct component as broken"
/// belongs beside that caller, not on a public surface.
const WORLD_EXPORTS: &[(&str, &[&str])] = &[
    ("ui", &["render", "interaction"]),
    (
        "hooks",
        &[
            "hook-pre-turn",
            "hook-pre-tool-use",
            "hook-post-tool-use",
            "hook-post-turn-end",
            "hook-attention-required",
            "hook-session-close",
        ],
    ),
];

/// The interface names a component exports from this package.
///
/// `exports` is what Wasmtime reports off a component's type:
/// `lca:ext/<interface>@<version>`. Unversioned, sorted, deduplicated.
fn exported_fragments<'a>(exports: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut found: Vec<String> = exports
        .into_iter()
        .filter_map(|export| {
            let rest = export.strip_prefix("lca:ext/")?;
            let name = rest.split('@').next().unwrap_or(rest);
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The worlds a component's `fragments` show it exports, in `WORLDS` order.
fn exported_worlds(fragments: &[String]) -> Vec<lca_ext_abi::World> {
    lca_ext_abi::WORLDS
        .iter()
        .filter(|(name, _)| {
            WORLD_EXPORTS
                .iter()
                .find(|(world, _)| world == name)
                .is_some_and(|(_, interfaces)| {
                    interfaces
                        .iter()
                        .any(|interface| fragments.iter().any(|f| f == interface))
                })
        })
        .map(|(_, world)| *world)
        .collect()
}

// Verifies: the fragment→world table agrees with the WIT definitions.
//
// The table exists so a Wasmtime-reported export name can be attributed to a
// world. A table that disagrees with `wit/` would misreport a correct component,
// so the disagreement is a failure here rather than a confusing warning later.
#[test]
fn the_export_fragment_table_matches_the_wit_worlds() {
    let hooks_wit = read("wit/world-hooks.wit");
    let ui_wit = read("wit/world-ui.wit");
    // The six hook interfaces the `hooks` world exports, read from the WIT.
    let mut hooks_exports: Vec<String> = hooks_wit
        .lines()
        .filter_map(|line| line.trim().strip_prefix("export "))
        .map(|name| name.trim_end_matches(';').trim().to_string())
        .collect();
    hooks_exports.sort();
    assert!(
        !hooks_exports.is_empty(),
        "the hooks world exports something"
    );
    // `hook-session-close` is the one this issue is about, and the absence of it
    // from the `ui` world is the whole contract.
    assert!(
        hooks_exports
            .iter()
            .any(|name| name == "hook-session-close"),
        "`hook-session-close` is a `hooks` export: {hooks_exports:?}"
    );
    assert!(
        !ui_wit.contains("hook-session-close"),
        "the `ui` world must not export it - that is why a ui-only hook is dead"
    );
    let ui_exports: Vec<String> = ui_wit
        .lines()
        .filter_map(|line| line.trim().strip_prefix("export "))
        .map(|name| name.trim_end_matches(';').trim().to_string())
        .collect();
    assert_eq!(
        ui_exports,
        vec!["render".to_string(), "interaction".to_string()],
        "the ui world exports exactly its two interfaces"
    );
}

// Verifies: the loader's world-attribution predicate, on real export lists.
//
// This is the WASM half of the same question, and it runs the predicate the
// loader uses rather than a copy: the names below are what
// `wasmtime::component::ComponentType::exports` really reports for the shipped
// components (measured with a probe, not assumed).
#[test]
fn a_component_exporting_a_hook_world_is_detected_from_its_exports() {
    // The reference ui component's real export set.
    let ui_component = ["lca:ext/render@0.6.0", "lca:ext/interaction@0.6.0"];
    let fragments = exported_fragments(ui_component);
    assert_eq!(fragments, vec!["interaction", "render"]);
    let worlds = exported_worlds(&fragments);
    assert_eq!(
        worlds,
        vec![lca_ext_abi::World::Ui],
        "a ui component is attributed to the ui world only - no hook world, so a \
         manifest declaring only ui contradicts nothing"
    );

    // A component that also carries the session-close hook - the shape that would
    // be dead under a `worlds = [\"ui\"]` manifest.
    let contradicting = [
        "lca:ext/render@0.6.0",
        "lca:ext/interaction@0.6.0",
        "lca:ext/hook-session-close@0.6.0",
    ];
    let fragments = exported_fragments(contradicting);
    let worlds = exported_worlds(&fragments);
    assert!(
        worlds.contains(&lca_ext_abi::World::Hooks),
        "the hook export is attributed to the hooks world: {worlds:?}"
    );
    assert!(
        worlds.contains(&lca_ext_abi::World::Ui),
        "and the render export to the ui world: {worlds:?}"
    );
}
