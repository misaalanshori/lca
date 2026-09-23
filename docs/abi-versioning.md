# ABI versioning and compatibility policy

Version 0.1, 2026-09-20.

This document defines what the extension ABI promises, what counts as a breaking change, how long an old version keeps working, and how a change gets made. It governs the `lca:ext` WIT package and the host imports under `lca:host`.

The policy exists because an extension is built once and run by hosts the author never sees. Without a written rule for what can change, every host release is a guess for every author.

## What the ABI covers

The ABI is the `lca:ext` WIT package: the worlds `provider`, `tool`, `command`, `hooks`, `ui`, `compaction`, and `context-transform`, and the host import interfaces they reference. It also covers the manifest schema, because a manifest the host cannot parse is a load failure like any other.

The ABI does not cover the internal crates. `lca-core` and `lca-session` change freely. It does not cover the terminal interface, the configuration file format, or the command line, which have their own compatibility rules in the release policy.

## Version numbers

The ABI uses semantic versioning, and the numbers mean specific things here.

A major version change breaks existing extensions. They stop loading and need a source change to work again.

A minor version change adds surface. Existing extensions keep working. New extensions built against the new minor version do not load on older hosts.

A patch version change fixes documentation, comments, or tooling. The interface bytes do not change.

The manifest declares a line as `major.minor`. An extension declaring `abi = "0.1"` targets any 0.1.x.

During 0.x, the minor position behaves as the breaking position, which is the normal semver convention for pre-1.0 and is why the ABI freezes at 1.0 in Phase 8.

## What breaks and what does not

The Component Model canonical ABI decides most of this, not taste. The following table is the working rule.

| Change | Effect |
|---|---|
| Add a new world to the package | Additive |
| Add a new function to an existing world | Breaking for that world (see the optional-export rule) |
| Remove or rename an exported function | Breaking |
| Add a case to an existing variant | Breaking |
| Add a field to an existing record | Breaking |
| Change a parameter or return type | Breaking |
| Add a new host import interface | Additive |
| Add a function to an existing host import interface | Additive |
| Remove a host import function | Breaking |
| Add a value to a string-typed vocabulary validated by the host | Additive |
| Add a value to a WIT enum | Breaking |
| Change documentation or comments | Patch |

Two rows deserve attention because they drive design.

Adding a case to a variant is breaking. This is why the provider event stream carries `vendor-event` and the widget tree reserves an extension point. Both exist so that new concepts travel through an existing case instead of forcing a major version. Any new variant in the ABI should include a similar case before the freeze.

Adding a value to a host-validated string vocabulary is additive. This is why filesystem scope names are strings rather than a WIT enum. New scopes ship in a patch release. This pattern is preferred for any vocabulary expected to grow.

Adding a function to a world is breaking because a component that does not export it fails to satisfy the world. Two shapes exist for growing a world's surface. A function every implementor exports unconditionally, returning a defined not-supported result where it does not apply, is the optional-export pattern that `login`, `logout`, and `usage` use on the `provider` world; such a function can only be added before the freeze, and after 1.0 adding one is an ordinary breaking change. A function a component may omit entirely must live in a separate world that an extension opts into by declaring it in the manifest.

Record growth has its own rule. Records that cross the ABI boundary, messages, usage, tool calls, and tool results, carry a reserved `extras` map of string pairs. New, non-structural data rides in `extras` in a minor or patch release; anything that changes a record's shape remains breaking. The freeze gate checks that every ABI-crossing record has `extras`.

## Support window

The host loads extensions built against the current ABI minor version and the one immediately before it.

This gives an author one minor cycle to rebuild and publish. It gives a user a host upgrade that does not silently disable half their extensions.

An extension targeting an older version than the window allows is refused at load time. The host disables it for the session, reports on a best-effort non-blocking check whether a compatible version exists in the registry, names the command that fixes it, and continues. A stale extension does not stop the agent from starting.

The window is tested, not assumed. The pipeline builds the conformance extension against the previous ABI minor version and asserts that the current host loads it and passes the suite.

## Deprecation

A function or type that will be removed in the next major version is marked deprecated in the WIT comments and in the ABI changelog when the decision is made, not when the removal happens.

A deprecated item keeps working for the whole of the current major version. It is removed only at a major boundary.

The changelog entry for a deprecation names the replacement. A deprecation with no replacement is a design problem, not a documentation task.

## The changelog

The ABI has its own changelog, separate from the agent's. It lives at `wit/CHANGELOG.md` and is written for extension authors.

Each entry names the version, the date, and every change grouped as added, deprecated, removed, or fixed. Each change says whether it is breaking and what an author has to do. An entry with no migration note for a breaking change is incomplete.

The changelog is updated in the same pull request as the WIT change. A separate documentation pass loses the reasoning.

## Making a change

A change to the ABI needs, in one pull request: the WIT edit, the regenerated bindings, a changelog entry, an update to the conformance extension covering the new or changed surface, an update to the manifest schema if the manifest is affected, and a version bump following the table above.

An ABI change also needs an ADR when it changes a design decision rather than filling in an agreed shape. Adding a capability is an ADR. Adding a field to a capability that an ADR already described is not.

The reviewer checks one thing above the rest: whether the change is breaking under the table, and whether the version bump matches. A breaking change with a minor bump is the failure mode that costs the most later.

## Host version reporting

`lca --version` prints the agent version, the ABI version, and the build target. An author debugging a load failure needs all three in one line.

The host exposes the same information to extensions through the always-granted log interface context, so an extension can record what it is running against.

## Freezing at 1.0

Phase 8 freezes the ABI at 1.0 (executed 2026-09-23: the WIT package and the `lca:host` imports both carry `@1.0.0`, `lca-ext-abi::ABI_VERSION` reads `1.0`, every first-party manifest declares `abi = "1.0"`, and `wit/CHANGELOG.md` opens with the freeze entry). After the freeze, no breaking change ships without a 2.0, and a 2.0 is a serious undertaking that needs its own plan for dual-loading or migration.

One amnesty comes with the bump: the line that was current at the freeze, 0.1, keeps loading on a 1.0 host. The window accepts it alongside the current line and the previous minor, so an extension published the week before the freeze does not die to a version change that altered no bytes. Nothing older than that line loads, and nothing about it applies to a future major.

Everything that should be a variant case, an extension point, or a string vocabulary rather than an enum has to be settled before the freeze. The conformance extension has to cover every surface. These are the two gates on the freeze, and neither is a matter of judgment at the time: they are checked.

The known punch list going into Phase 8, each already decided and awaiting implementation rather than still open: the `provider` world gains `login`, `logout`, and `usage` as defined optional exports, per ADR-0012. Two new worlds, `compaction` and `context-transform`, join the five already specified, per ADR-0015. The `completion` capability, and the `net-local` and `pty` capabilities, join the catalog, per ADR-0015, ADR-0011, and ADR-0016 respectively. The `usage` event also gains `cost`, every ABI-crossing record gains `extras`, the widget vocabulary gains `image`, and the hook points are fixed at `pre-turn`, `pre-tool-use`, `post-tool-use`, `post-turn-end`, `attention-required`, and `session-close`. None of these are open questions; they are scoped work items the freeze gate checks for completeness, not decisions the freeze itself needs to make.

## Pre-1.0 reality

Before 1.0, minor versions break. The support window still applies, so 0.2 loads 0.1 extensions, but an author should expect to rebuild each cycle.

Authors publishing during 0.x should track the changelog and push a rebuilt artifact within one cycle of each minor release. The ABI line tag in the registry makes this mechanical: a rebuild is a new push under a new `abi-0.N` tag.
