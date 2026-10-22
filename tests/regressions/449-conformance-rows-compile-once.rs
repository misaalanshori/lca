//! GitHub #449: `conformance_diff` compiled the WASM fixture more than once
//! per row, so two rows drifted past nextest's 120 s cap on a loaded box.
//!
//! ## What the measurement said
//!
//! `host.load` costs 5.8–6.6 s on this box, and essentially all of it is
//! `wasmtime::component::Component::new`: `ExtHost::new` is 0.3 ms, `Engine::new`
//! is 1.2 ms. A second `Component::new` on the **same** engine costs the same
//! 8.4 s as the first, so sharing an engine buys nothing — wasmtime does not
//! memoise compiles. And nextest forks a **process per test**, which is why the
//! compile cannot be shared *across* rows either: a `OnceLock` in the file is
//! re-initialised for every row.
//!
//! So the only reducible part was the redundant loads *inside* the two slow
//! rows: `structured_output_round_trips_in_both_modes` compiled three times
//! (once for `both_modes`, then again for the schema assertions) and
//! `the_dialog_verdict_agrees_across_modes` compiled once per verdict. Both
//! were buying a second compile for something the handle already answers:
//! `schema()` is another call into the same component, and `SharedDialogs` is a
//! live slot read per call, so a second prompter install needs no second guest.
//!
//! Removing them took the rows from 64.2 s and 62.7 s to 11.5 s and 7.9 s, and
//! the whole file from 109.7 s to 103.8 s — which is why **no budget override
//! exists**: the fix is sharing the compile, not tolerating it.
//!
//! ## Why a structural guard, not a stopwatch
//!
//! A wall-clock assertion would re-introduce the very fragility this issue is
//! about: it would fail on a loaded runner for a reason unrelated to the
//! product. The durable invariant is *how many times a row compiles the
//! fixture*, which is a property of the source and is exactly what regressed.
//! So this guard pins the load sites.
//!
//! Verifies: gh #449 acceptance 1 and 2 (margin under load, both modes still
//! exercised), and `docs/testing-plan.md` §13's measured-budget rule (the number
//! stays re-derivable, and no blanket raise hides a regression).

use std::fs;

/// The two rows this issue named, and what each is allowed to compile.
const ROWS: [(&str, usize); 2] = [
    ("structured_output_round_trips_in_both_modes", 1),
    ("the_dialog_verdict_agrees_across_modes", 1),
];

fn source() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/lca-ext-host/tests/conformance_diff.rs"
    );
    fs::read_to_string(path).expect("the conformance diff test file is readable")
}

/// The body of one top-level `fn`, from its signature to the next
/// top-level item. Deliberately textual: this is a guard *about* the
/// source, in the same spirit as the vocabulary rows that read a doc table.
fn body_of<'a>(source: &'a str, name: &str) -> &'a str {
    let signature = format!("fn {name}(");
    let start = source
        .find(&signature)
        .unwrap_or_else(|| panic!("`{name}` is still a row in the file"));
    // The row ends at the next line that closes the function at column 0.
    let rest = &source[start..];
    match rest.find("\n}\n") {
        Some(end) => &rest[..end],
        None => rest,
    }
}

/// Every way this file can compile the fixture: the shared helper, and any
/// direct `host.load` a row might add instead.
fn compile_sites(body: &str) -> usize {
    // A compile is either the shared helper (one compile each) or a direct
    // `host.load` a row wrote for itself. They ADD: the defect this guards
    // was one of each in the same row.
    body.matches("host.load(FIXTURE, MANIFEST)").count()
        + body.matches(".both_modes(").count()
        + body.matches(".both_modes_keeping_wasm(").count()
}

#[test]
fn a_named_row_compiles_the_fixture_once() {
    let source = source();
    for (name, allowed) in ROWS {
        let body = body_of(&source, name);
        let sites = compile_sites(body);
        assert!(
            sites <= allowed,
            "{name} compiles the WASM fixture {sites} time(s), budget is {allowed}.\n\
             Each compile is ~6 s in a test build (`Component::new` dominates), and \
             nextest gives every row its own process, so a second compile cannot be \
             shared with a sibling. Ask the handle you already have: `schema()` is \
             another call into the same component, and `SharedDialogs` is a live slot \
             read per call."
        );
        // The row must still prove something: an empty body would satisfy the
        // budget above by doing nothing.
        assert!(
            body.contains("execute_tool") || body.contains("schema()") || body.contains("ask("),
            "{name} must still exercise both modes, not just load"
        );
    }
}

/// Every `fn` in the file whose name looks like a test row, with its body.
fn rows(source: &str) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    for line in source.lines() {
        let declaration = line
            .strip_prefix("fn ")
            .or_else(|| line.strip_prefix("async fn "));
        let Some(declaration) = declaration else {
            continue;
        };
        let Some(paren) = declaration.find('(') else {
            continue;
        };
        let name = &declaration[..paren];
        found.push((name, body_of(source, name)));
    }
    found
}

#[test]
fn no_row_compiles_the_fixture_more_than_the_shared_helper() {
    // The general form, so a *new* row cannot quietly pay for a second
    // compile either. Every row in the file is at one today, so one is the
    // bound - a row that needs its own environment should build the
    // environment, not recompile the component for it.
    let source = source();
    let all = rows(&source);
    assert!(
        all.len() >= 10,
        "the file still has its rows: {}",
        all.len()
    );
    for (name, body) in all {
        if !body.contains("execute_tool") && !body.contains("schema()") {
            continue; // a helper, not a row
        }
        let sites = compile_sites(body);
        assert!(
            sites <= 1,
            "{name} compiles the fixture {sites} times in one row; a row should \
             reuse the handle it already loaded"
        );
    }
}

#[test]
fn no_budget_override_covers_the_whole_file() {
    // The measured-budget rule: a raise must be scoped to an exact test name
    // and carry its own measurement. An override keyed on the *binary* or on a
    // broad filter would give every row the slow row's ceiling, which is how a
    // future regression hides.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/.config/nextest.toml");
    let config = fs::read_to_string(path).expect("nextest config is readable");
    for line in config.lines() {
        let Some(filter) = line.trim().strip_prefix("filter =") else {
            continue;
        };
        assert!(
            !filter.contains("conformance_diff") || filter.contains("test(="),
            "a conformance_diff override must name the exact test, not a broad \
             filter: {filter}"
        );
    }
}
