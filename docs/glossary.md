# Glossary

Version 0.1, 2026-09-20.

Terms as used across this document set specifically. Several of these have a broader meaning elsewhere in software; where that's likely to cause confusion, the broader sense is named and set aside.

**ABI.** Here, specifically the `lca:ext` WIT package: the set of worlds and host imports an extension is built against. Not the platform calling-convention sense of the term, which this project also has, incidentally, at the level of the Rust toolchain, but never discusses under this name.

**Ad hoc grant.** A single specific path or host the user attaches to an extension's grant outside the manifest's fixed vocabulary, at install time or later, with consent text naming the exact path or host. A manifest can never request one; see `docs/capabilities.md` and FR-PERM-16.

**Build-time backend.** In the sense of ADR-0013, code with more than one implementation, all supplied by the project and chosen when the binary or web bundle is built: no manifest, no consent screen. The read, write, and shell tools are the clearest case. Distinct from both fixed core and a runtime extension.

**Capability.** A named, manifest-declared grant an extension holds: `net`, `net-local`, `fs`, `credentials`, `oauth`, `process`, `pty`, `ui`, or `completion`. The host links every capability interface a world carries in a denied state; an ungranted call returns a recorded permission error (FR-PERM-3), and an interface a world does not import at all is absent from the link. See `docs/capabilities.md`.

**Compaction.** The mechanism that replaces an old range of session records with a summary when usage crosses a threshold, implemented by a `compaction`-world extension and written back as a durable log record. Not a general synonym for shrinking data; a `context-transform` extension also shrinks what goes out on the wire but is never called compaction, because it touches nothing durable. See ADR-0015.

**Component.** A WebAssembly Component Model artifact: a `.wasm` file conforming to the Component Model, built with typed imports and exports described by WIT, as distinct from a plain "core" WebAssembly module with no such typing. Every extension is a component. When this document set says "component" without qualification, this is the sense meant.

**Conformance extension.** The extension under `extensions/conformance/` that exercises every world and every host import, including denied paths, and is run in both native-linked and WASM mode with the two results diffed. Not a general term for any extension that happens to be well-tested.

**Context-transform.** The world that reshapes the outbound message list on every turn without touching the session log. See `compaction` above for the distinction, and ADR-0015 and ADR-0017 for how the two interact around the cache boundary.

**Core.** In the narrow sense from ADR-0013, code with exactly one implementation and no extension point at all: the agent loop's shape, the session log's framing, the permission enforcement path. Not a loose synonym for "the main codebase" or "the important part"; a build-time backend, such as the read and write tools, is not core in this sense even though it ships unconditionally, because it has more than one implementation selected at build time.

**Data-only extension.** A package with `worlds = []` and no component: a manifest and a `resources` bag, nothing else. Installs and removes through the same pipeline as any other package, and the loader skips registering it because there is nothing to register. A skill pack is the common case. See ADR-0030 and ADR-0032.

**Delivery mode.** Whether a given extension instance runs native-linked, compiled directly into the binary with no sandbox, or as a WASM component under the capability-enforced host. A single extension's source can support both; delivery mode is a build-time or install-time choice, not a property of the source itself. See ADR-0002 and ADR-0013.

**Dynamic suffix.** The portion of a resolved message list after the stable prefix: the turns since the most recent compaction. See `stable prefix` below.

**Extension.** A runtime-installed, capability-gated unit of behavior implementing one or more WIT worlds, in either delivery mode. Not every pluggable piece of the system is an extension in this sense; see `core` and the build-time-backend sense under `delivery mode`. See ADR-0013 for the full three-way distinction.

**Grant.** The result of resolving an extension's declared capabilities against what the user has approved; the actual, enforced permission set an instance runs with, as opposed to what its manifest merely asked for. A manifest can declare more than it's granted; the import table reflects the grant, not the declaration.

**Guest.** The extension's own code, from the perspective of the WASM runtime hosting it. The counterpart term is `host` below. "Guest" and "extension" are near-synonyms in casual use; "guest" specifically emphasizes the WASM-runtime relationship, which matters when discussing something like the sibling-instantiation hosting model in ADR-0018, where there are two guests, the agent and an extension, sharing one host engine.

**Harness.** The shared test infrastructure in `lca-testkit`, including the fake provider and the sandboxed test environment, that integration and end-to-end tests are written against. See `docs/testing-plan.md`.

**Hook.** A registered callback at a defined point in the agent loop: before a turn, before a tool call, after a tool call, after a turn, when attention is required, or when a session closes. Only the pre-tool hook can alter behavior, returning allow, deny, or replace; every other hook point observes. Not the mechanism `context-transform` uses, even though both run on every turn in similar places in the loop; see ADR-0015 for why they're kept separate.

**Host.** The `lca` process itself, from the perspective of an extension running inside it: the thing that instantiates a component, builds its import table, and implements every host import the extension calls. Not "host operating system," though the two senses are related; where ambiguity is possible, "host process" or "host OS" is used instead.

**Interpreter, JIT, AOT.** The three ways a WASM runtime can execute a component. An interpreter walks bytecode with no code generation and needs no executable memory. A JIT compiles to native code at load or call time and needs executable memory, which some environments forbid. AOT compiles once, ahead of time, to a cached native artifact that runs like ordinary native code afterward with no runtime compiler resident. See ADR-0001.

**Lockfile.** The extension lockfile `lca-registry` owns, recording each installed extension's resolved digest, source reference, and approved-capability hash. Not `Cargo.lock`, which also exists in this project and means the ordinary Rust-ecosystem thing; where both could be meant, "extension lockfile" and "Cargo lockfile" are used explicitly.

**Login surface.** The `provider-login` export (ADR-0033): `login-options` returns the picker choices the host renders, and `login-submit` consumes the user's answers, stores the secret in the extension's own credentials namespace, and returns opaque `setting: value` pairs the host persists. The host is UI, courier, and consent only - it never interprets a preset's shape. Not the same as `login`, which is the extension's own self-contained authentication flow (an OAuth dance).

**Manifest.** The TOML file, `extension.toml`, declaring an extension's identity, ABI target, implemented worlds, and requested capabilities. The install-time consent surface; see `schemas/extension-manifest.schema.json`.

**OCI artifact.** A component and its manifest, published to any registry implementing the OCI Distribution Specification, resolved by reference and pinned by digest. One of the source kinds `lca-registry` understands; see ADR-0010 for the others.

**Preopen.** A WASI term: a directory handle an extension receives already opened and scoped by the host, so the extension resolves paths relative to a handle it was given rather than an absolute path it constructed itself. The mechanism underneath every `fs` capability grant.

**Preset.** A named endpoint entry an extension ships in its own `resources/provider-presets.toml`: id, display name, base URL, auth kind, curated model list. Extension data, not host data - disabling the extension takes its presets with it. The host's `login-options` query maps presets to picker rows and nothing more. A user's own presets live at `<config>/provider-presets.toml`. See ADR-0031.

**Provider.** An extension implementing the `provider` world: model listing, streaming completions, authentication, and the `login`, `logout`, and `usage` exports from ADR-0012. Not a synonym for "vendor" or "API"; a single vendor's API is what a provider extension talks to, not what the term itself names.

**Resources (bag).** An extension's own read-only package data, served by `lca:host/resources` from `resources/` and visible only to the extension that owns it - never the filesystem, never another extension. Not "system resources" (memory, handles) and not the everyday plural of "resource" in a URL sense; where ambiguity is possible, "the resources bag" is used. The host also reads some kinds for its own features, notably `resources/skills/<name>/SKILL.md`. See ADR-0030.

**Scope.** In the `fs` capability specifically, one of the named vocabulary entries, `workspace`, `private`, `home-config`, or `temp`, that a manifest grants read or write access to. See `workspace` below for a term collision worth knowing about.

**Session.** One durable, append-only conversation record on disk, with its own log, its own metadata, and its own identity, forkable and resumable. See `docs/session-log-format.md`.

**Stable prefix.** The leading portion of a resolved message list, up to and including the most recent compaction, that a provider's prompt cache can reuse across turns unchanged. The host computes this boundary and passes it to the active provider extension on every completion call. See ADR-0017.

**State (bag).** An extension's own mutable, non-secret data, served by `lca:host/state` from `<state_dir>/state/<name>/`: caches, last-used values, counters. Keyed by the extension's own identity so a cross-namespace read has no address; size-capped; wiped on uninstall. Not secret-grade - secrets go in `credentials`. Not session state and not the `stable prefix`/`dynamic suffix` sense of "state" used elsewhere. See ADR-0030.

**Turn.** One round of the agent loop: a user or system input, a model's response, any tool calls that response triggers and their results, repeated until the model stops without requesting a tool. Not the same as a single model API call; a turn with three sequential tool calls involves four calls to the provider.

**Vendor-event.** The reserved, open-ended case in the provider stream's typed event variant, carrying a vendor-specific kind string and a JSON payload, for anything the other typed cases don't cover. Exists specifically so a new vendor concept doesn't force an ABI break; see ADR-0004.

**WIT.** WebAssembly Interface Types, the interface-description language the Component Model uses to define worlds and their imports and exports. The `.wit` files under `wit/` are the normative source for the extension ABI's shape.

**World.** A named bundle of typed functions a component can export: `provider`, `tool`, `command`, `hooks`, `ui`, `compaction`, or `context-transform`. An extension implements as many worlds as it needs; nothing about implementing one constrains which others it can also implement.

**Workspace.** Two distinct things share this word, and context is what disambiguates them. The `workspace` scope, lowercase, unqualified, is the `fs` capability's name for the current project's root directory, defined in `docs/capabilities.md`. The Cargo workspace, always paired with "Cargo" in this document set when meant, is the single top-level `Cargo.toml` that ties together every crate under `crates/` and `extensions/`, defined in ADR-0002. The two are unrelated; a reader who sees "workspace" without "Cargo" attached should assume the `fs` scope is meant.
