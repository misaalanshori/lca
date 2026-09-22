# Software testing plan

Version 0.1, 2026-09-20.

This document specifies how LCA is tested: what test-driven development means for this project specifically, the taxonomy of test types and what each covers, the fake-provider strategy that makes the agent loop testable without a real model, and a dedicated treatment of cache-behavior testing, since keeping a provider's prompt cache warm is close to a functional requirement for a coding agent, not a performance nicety. It is a companion to the software requirements and design document, not a restatement of it; requirement identifiers referenced here are defined there.

## 1. Why test-driven development, specifically here

The usual case for TDD is a human one: writing the test first clarifies the requirement before the implementer commits to an approach, and the resulting suite catches regressions a human would otherwise reintroduce by forgetting an old constraint. Both apply here. A third reason applies more sharply than usual, because of who is expected to write most of this code.

An AI agent implementing a feature in a fresh session has no memory of the design discussion that produced the requirement, no tacit sense of "we tried that and it broke something," and no discomfort about producing code that looks plausible but is subtly wrong in a way a human reviewer would catch on sight. A failing test is a piece of ground truth that doesn't depend on the implementer's judgment agreeing with the reviewer's. Red, green, refactor is not a style preference in this context; it's the mechanism by which a requirement survives being handed to an implementer with no memory of why it exists.

This has a direct consequence for how tests get written: a test should be traceable to a requirement identifier, an ADR, or a defect identifier, not written free-form against the author's own understanding of what the code should do. Section 11 specifies the mechanism that keeps this traceable rather than aspirational.

## 2. Test taxonomy

Nine categories. Each has a home in the crate layout, a tool, and a place in the CI pipeline described in section 13.

**Unit tests** verify one function or one small module in isolation, live beside the code they test in the same file under `#[cfg(test)]`, following the Rust convention, and run in milliseconds. Every crate has them; there is no crate where unit tests are optional.

**Integration tests** verify a flow that crosses module or crate boundaries within a single process, live under each crate's `tests/` directory, and use `lca-testkit`'s harness and fake provider rather than a real model or real network access. The six flows diagrammed in `docs/flows.md` are the primary integration test targets: a turn with a tool call, context assembly with compaction and transform, extension instantiation and capability resolution, the OAuth loopback flow, the streaming pipeline, and cancellation reaching a running extension call.

**Conformance tests** verify the extension ABI itself, not any particular extension. The conformance extension under `extensions/conformance/` exports and imports every surface defined in every WIT world and every host capability, and is run in both native-linked and WASM mode with an assertion that the two produce identical results, per FR-EXT-6. It is also built against the previous ABI minor version and loaded by the current host, verifying the support window in NFR-19 actually holds rather than being an assumption nobody checks.

**Regression tests** exist because a defect that reached a release and got fixed should never come back silently. Each one is named after the defect's tracking identifier, following pi's convention of naming regression test files directly after the issue number they close, and lives under `tests/regressions/`. NFR-24 already requires one per released defect; this document specifies the naming and location.

**Property-based tests**, using `proptest`, verify an invariant across a generated range of inputs rather than one hand-picked example. This project has several state-machine-shaped invariants well suited to it: session log round-tripping, compaction never dropping a record outside its replaced range, the context-transform chain preserving message order except where a transform explicitly changes it, and the permission store's proposal-hash comparison detecting any single-byte change to the approved set.

**Fuzz targets** verify that a parser handling attacker-influenced input never panics or produces undefined behavior, following the threat model's own list: the manifest parser, the session log reader, the HTTPS-archive resolver, and the canonical ABI decode path.

**Snapshot tests** verify that a piece of generated output, a rendered terminal frame or an assembled request, matches a reviewed golden file, so a change to either shows up as a diff a human approves rather than as a passing test that silently started asserting something different. Section 10 covers the specific and unusual use of this technique for catching cache-breaking regressions.

**Performance and benchmark tests**, using `criterion`, verify the numeric NFRs: binary size, cold start, instantiation time, hook call overhead, idle memory, and cancellation latency. These run in the pipeline's size-and-startup gate described in the release policy, and a threshold breach fails the build the same way a failing test does.

**End-to-end tests** drive the actual `lca` binary as a subprocess, exercising the real CLI, the real filesystem, and the real terminal rendering path, still against the fake provider rather than a real model. These are the slowest and fewest tests in the suite and exist specifically to catch the class of defect that only shows up when every layer is real except the model.

## 3. The fake provider

`lca-testkit`'s fake provider is the single most load-bearing piece of test infrastructure in the project, modeled directly on pi's `providers/faux.ts`: a provider implementation that speaks the real `provider` world ABI and returns scripted content in a defined order, so every test exercising the agent loop, compaction, retries, cancellation, or tool dispatch runs deterministically, at zero cost, with no network access, through the actual dispatch path rather than a shortcut around it.

A test scripts a fake turn as an ordered sequence of events matching the typed provider stream from ADR-0004: text deltas, reasoning deltas, a tool call's start, argument deltas, and end, a usage event, or an error, in whatever order the scenario calls for. The harness exposes a small builder so a common case, "the model replies with this text and calls this tool with these arguments," is a few lines, not a hand-assembled event stream.

```rust
let fake = FakeProvider::builder()
    .turn(|t| t
        .tool_call("read", r#"{"path":"notes.md"}"#)
        .usage(Usage { input: 1200, output: 40, cache_read: 0, cache_write: 1200, ..Default::default() }))
    .turn(|t| t
        .text("The file contains three notes.")
        .usage(Usage { input: 1240, output: 12, cache_read: 1200, cache_write: 0, ..Default::default() }))
    .build();
```

Every fake turn carries a usage record, including cache read and write counts, per ADR-0017. This is deliberate and non-optional in the builder: a test that doesn't care about cache behavior can leave the fields at zero, but a test that scripts a realistic multi-turn conversation without cache numbers is scripting something that couldn't happen against a real caching provider, and section 9 depends on being able to script exactly this.

The fake provider also simulates a capability-denied path, an error mid-stream, and a stream that ends with a tool call still open, since these are the failure paths FR-PROV-8 and the accumulator tests need and a real provider will not reliably reproduce on demand.

## 4. The harness convention

Following pi's own `test/suite/` convention directly: integration tests use a shared harness, never call a real provider, a real API key, or the network, and stay deterministic enough to run in CI without flakiness. The rule, adapted from pi's own stated policy: use the fake provider, do not extend a legacy ad hoc setup path once the harness covers the need, and if a test genuinely cannot be expressed against the fake provider, that is a signal the harness is missing a capability, not a reason to reach for a real one.

Real-provider tests do exist, one per first-party provider extension under `docs/providers/`, and are gated behind an environment variable naming the credential they need, skipped rather than failed when it is absent, matching pi's own pattern of never blocking a contributor's local run or the default CI job on a credential they don't have.

## 5. Test environment sandboxing

Every test that touches the filesystem or environment runs inside an isolated temporary `HOME` and config directory, constructed fresh per test, with no access to the developer's real credentials, git config, or shell environment. This follows the sandboxing pi's own `test.sh` performs before running its suite: an isolated `HOME`, an isolated package cache, a stripped environment, and `GIT_ASKPASS` disabled so nothing can prompt for real credentials by accident. `lca-testkit` provides this as a fixture every integration and end-to-end test gets by default, not as something each test author has to remember to set up.

## 6. Conformance testing in depth

The conformance extension is not a toy; it is the thing that makes the promise "native-linked and WASM are interchangeable delivery modes" a tested fact rather than an assertion in a document. It needs a case for every world, every host import, and every capability's denied path, not just the happy path:

Every world's every exported function, called once with valid input and asserted against an expected result. Every host import's granted path and its denied path, including the specific denial shapes that differ by capability: `credentials.get` returning empty rather than an error, `net` refusing a rebinding attempt distinctly from an ordinary denial, `fs` refusing a path that escapes its scope through a symlink created after the grant. The `pty` and `completion` capabilities' full grant-and-deny cycle, since these are the newest and least battle-tested parts of the catalog. The `compaction` and `context-transform` worlds' rejection paths, and the boundary-narrowing behavior from ADR-0017 when a transform touches the stable region.

This runs in native-linked mode and WASM mode on every pipeline run, with the two results diffed; any divergence is a defect regardless of which mode is "wrong," since the entire point of the dual-mode design is that there shouldn't be a wrong one.

## 7. Regression test policy in depth

A defect that reaches a released build gets a test that reproduces the minimal case, named after the issue that tracks it, and placed under `tests/regressions/`, following pi's own directory of numbered regression files directly. The file name carries the issue identifier and a short description, for example `1842-compaction-drops-tool-result.rs`, so the file itself documents which bug it guards against without needing to open it.

A regression test is written before the fix, following the same red-green discipline as any other TDD work: the test reproduces the defect and fails against the unfixed code, then the fix makes it pass. A pull request that fixes a defect without a regression test in the same change does not merge; this is a coding-standards rule, not a suggestion.

## 8. Property-based testing

Four invariants are specified here as the initial `proptest` target set; more are added as new stateful logic is written, following the same bar: an invariant that should hold across every input, not just the examples a unit test happened to pick.

Session log round-tripping: any sequence of valid records, written and then read back, produces an identical sequence, and a log truncated at any byte offset produces the longest valid prefix rather than an error, matching the recovery behavior specified in `docs/session-log-format.md`.

Compaction range integrity: a compaction extension's declared replaced range, applied to any generated session history, never causes a reader to lose a record outside that range, and a nested compaction, one whose range includes an earlier compaction marker, still resolves to a single coherent view.

Context-transform chain ordering: for any generated chain of transform extensions and any generated message list, the chain's output preserves the input's order except where a transform in the chain explicitly reorders, and a rejection at any position in the chain stops every transform after it from running.

Permission proposal hashing: for any generated proposal set and any single-field mutation of it, the stored hash comparison in ADR-0006's split store detects the change and triggers the difference prompt; it never silently accepts a mutated set as unchanged.

## 9. Cache behavior testing

This is the section the cache-preservation design in ADR-0017 exists to make testable, and it is worth treating as its own concern rather than folding into the general integration test list, given how central prompt caching is to what makes a coding agent affordable and responsive to use.

### What gets tested and how

Every scenario below scripts the fake provider with realistic usage numbers, runs a multi-turn session against it through the real agent loop, and asserts on the cache-waste figures the module from ADR-0017 computes from those numbers, the same way a real deployment would compute them from a real provider's responses. Nothing here needs the host to inspect request bytes; it needs the fake provider to report numbers a real provider would report, and the waste-detection logic to interpret them correctly.

A clean multi-turn conversation, no compaction, no model switch, no extension changes between turns, reports zero cache waste from the second turn onward. This is the baseline every other scenario is a deliberate deviation from.

A conversation that crosses the compaction threshold reports the compaction turn's own cost normally, since that prompt is genuinely new, and resumes reporting zero waste from the turn after, confirming the baseline reset behavior lands exactly on the compaction record and nowhere else.

A conversation where the active provider is switched mid-session reports the switch turn as counted waste, not exempted, matching the explicit design choice in ADR-0017 and in pi's own source. A test asserting the opposite, that a switch is silently exempted, would be testing the wrong thing; this scenario exists specifically to keep that distinction from eroding under a later, well-intentioned "fix."

A conversation that installs or enables a new extension mid-session, changing the tool list the provider sees, reports exactly one counted miss on the turn after the change and zero waste on every turn after that, confirming that a tool-set change is treated as a real, one-time prefix change rather than as ongoing waste or as invisible.

A `context-transform` extension that only appends content after the stable boundary produces zero waste attributable to it. A second, deliberately misbehaving transform extension that mutates content inside the stable boundary triggers the boundary-narrowing and extension-event recording behavior from ADR-0017 on the first affected turn, and the test asserts that the event is recorded, that the turn completes rather than failing, and that the boundary narrows to end before the earliest differing message. A third scenario covers the settling case: a transform that rewrites stable content deterministically, identically on every turn, records exactly one divergence and stops narrowing once its output matches the previous turn's, because cross-turn stability is what the provider's cache keys on, not input-versus-output equality.

A provider that never reports any cache activity across an entire scripted session, simulating a vendor or local server with no caching concept, reports no waste at all rather than reporting every turn as a total miss, exercising the "nothing to measure" distinction from ADR-0017 directly.

A miss below the 1024-token noise floor, adopted from pi's own calibration, is not counted, confirmed by a scenario scripting a small, deliberate prompt-size fluctuation under that threshold and asserting the waste total stays at zero.

### Why this belongs in the pipeline, not just in a developer's local run

A cache-hit-ratio regression is invisible in ordinary functional testing: every one of these scenarios produces a correct answer from the model, a passing turn, and a happy user, right up until the bill or the latency reveals that caching silently stopped working days or weeks earlier. The clean-conversation scenario above is promoted to a benchmark, not just a test: it runs in the same pipeline gate that checks binary size and cold start, asserting the computed cache-hit ratio across a canonical twenty-turn scripted session stays above a threshold, and a regression here fails the build the same way a binary-size regression does.

## 10. Snapshot testing for cache-relevant assembly

Terminal rendering already uses snapshot tests against a virtual terminal buffer, per the software requirements and design document's own testing strategy. The same technique applies to something less obvious and specifically useful here: the assembled request.

For a canonical session state, fixed extensions, fixed history, no variation, the exact serialized message list and usage-relevant fields the host would hand to a provider's `stream-completion` call is snapshotted. A change to system prompt assembly, tool schema generation, or the transform chain that alters this snapshot shows up as a diff in review, and a reviewer sees plainly what changed and whether it was supposed to. This catches a category of regression the functional cache-waste tests above cannot: a change that would break a real provider's cache without ever showing up as a "miss" in a fake-provider test, because the fake provider was never told to care about the specific bytes that changed. The two techniques are complementary, not redundant: the cache-waste tests verify the waste-detection logic itself is correct, and the snapshot test verifies the thing actually being sent hasn't quietly drifted.

## 11. Requirements traceability

Every functional and non-functional requirement in the software requirements and design document should be verified by at least one test, and this is enforced mechanically rather than trusted to review discipline, since a missing test for a requirement is exactly the kind of gap that's invisible until it matters.

A test that verifies a specific requirement carries that requirement's identifier in a doc comment immediately above the test function, in the form `// Verifies: FR-CTX-3`. A CI check, run on every pull request, greps the full test tree for these markers, extracts every requirement identifier the SRDD defines, and fails the build if any FR or NFR has zero matching tests. It also flags, as a warning rather than a failure, any test marker referencing an identifier that no longer exists in the document, since a requirement that was renumbered or removed should have its test comment updated in the same change.

This does not replace the qualitative judgment of whether a test actually verifies what it claims to; it only guarantees that every requirement has something pointed at it, which is the mechanical half of the problem and the half most likely to silently rot without a check.

## 12. Definition of done

A change is done when: a failing test existed before the implementation and now passes; the test's doc comment references the requirement, ADR, or defect identifier it verifies; a defect fix carries a regression test under `tests/regressions/`; an ABI-surface change updates the conformance extension in the same change; documentation affected by the change is updated in the same change, per the coding standards already in the software requirements and design document; and, specifically for anything touching message assembly, the provider call, `context-transform`, or `compaction`, the change has been checked against the cache-behavior scenarios in this document, either by running them or by a written note explaining why they don't apply.

## 13. CI execution strategy

Unit and integration tests run on every push, on Linux, macOS, and Windows, using `cargo-nextest`. Conformance tests run on every push in both delivery modes. Property-based tests run a bounded case count on every push and an extended case count on a nightly schedule, since `proptest`'s value scales with iteration count and a full run on every push would slow the everyday feedback loop for no proportionate benefit. Fuzz targets run continuously on a dedicated schedule, not per push, with their corpora checked into the repository so a crash found once is a regression test forever after, not a one-time discovery. The cache-hit-ratio benchmark and the binary-size and cold-start gates run on every push to `main` and fail the build on a threshold breach, per the release policy. End-to-end tests, being the slowest category, run on every push to `main` and on every pull request marked ready for review, not on every intermediate commit.

A flaky test is quarantined, not ignored: it is marked and excluded from the required check within one working day of being identified, with a tracking issue, and a quarantined test past a set age without a fix blocks new quarantines from the same crate until it is resolved, so quarantining doesn't quietly become the normal way tests are handled.
