# LCA

A lightweight, cross-platform coding agent, written in Rust, distributed as a single static binary, extensible through sandboxed WebAssembly components instead of native plugins or an npm-style package system.

## What this is

A small trusted core handles the agent loop, session storage, the built-in file and shell tools, the terminal interface, and the permission system. Everything else, model providers, compaction, slash commands, lifecycle hooks, custom rendering, is an extension: a WebAssembly component running under an enforced capability model, or, for a handful of first-party extensions that ship enabled by default, the same source compiled natively into the binary instead. The agent can run with zero providers installed as a valid state; it ships with one, speaking any OpenAI-compatible endpoint, so a fresh install has something to talk to.

The binary targets Linux, macOS, and Windows natively, and also builds to `wasm32` for embedding in a browser or Node host. Extensions install from an OCI registry, a plain HTTPS-hosted archive, or a local path, no npm and no system package manager required.

## Where this comes from

This is, in large part, a Rust port of [Pi](https://github.com/earendil-works/pi)'s design: its minimal-core philosophy, its approach to prompt compaction, its testing discipline, and specifically its method for measuring prompt-cache efficiency are all adopted directly, with attribution, throughout this document set. What Pi doesn't have, and this project adds, is a sandboxed, capability-gated extension boundary; Pi's own extensions run as unsandboxed code with full trust required.

The native-single-binary architecture and the proof that compiling the same agent to a native target and to `wasm32` for JS-host embedding both work come from studying [fx](https://github.com/vercel-labs/fx). fx's own extension surface is much thinner than what this project builds, no general third-party plugin API, no sandbox, so it's the secondary reference, mainly useful for the native-and-WASM half of the picture rather than the extension model itself.

The full account of what's taken from where, including specific files worth reading in each project's source, is in `docs/inspiration.md`. Read it before assuming either project's exact behavior can be guessed rather than checked.

## Status

Design-complete, pre-implementation. Every decision here has been written down as a requirement, an architecture decision record, or both, specifically so implementation can proceed without re-litigating settled questions or guessing at intent.

## Reading order

Start with `docs/lca-srdd.md`, which is the top-level requirements and architecture document and the index for everything else. From there: `docs/adr/` holds eighteen architecture decision records, one per real design choice with alternatives considered; `docs/capabilities.md` is the normative reference for every capability an extension can hold; `docs/testing-plan.md` specifies how this gets built test-first; `docs/glossary.md` disambiguates the terms that get overloaded across this many documents; `docs/platform-notes.md` has the specific, easy-to-get-wrong behavior per operating system; and `docs/providers/` documents each first-party model provider individually.

## If you are the agent implementing this

Read `docs/inspiration.md` and `docs/testing-plan.md` before writing any code. A few things worth holding in mind throughout:

The core stays small on purpose. If you find yourself adding a feature directly to `lca-core` or `lca-session` rather than as an extension, check `docs/adr/0013-three-kinds-of-pluggability.md` first; the default answer to "should this be core" is no.

Every requirement in the design document (`FR-*`, `NFR-*`) should have a failing test written against it before the implementation that satisfies it. `docs/testing-plan.md` explains why this matters more here than in ordinary human-paced development, not just that it's a rule.

Prompt-cache preservation is a functional requirement, not a nice-to-have; `docs/adr/0017-prompt-cache-preservation.md` and the cache-behavior section of the testing plan explain the mechanism and how to verify it hasn't regressed.

When a design question comes up that isn't already answered here, check how Pi handles it first, check fx only if the question is specifically about the native-binary or WASM-embedding side, and if neither has an answer, that's a signal to write a new ADR, not to guess and move on.
