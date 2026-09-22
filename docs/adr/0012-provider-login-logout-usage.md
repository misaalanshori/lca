# ADR-0012: Provider world gains login, logout, and usage

Status: accepted.

Date: 2026-09-20.

## Context

A user with several provider extensions installed wants two related things. A single `/login` that shows a picker of installed providers rather than a different login command per extension to remember, and a `/usage` that reports on whichever provider is currently active, while a namespaced form like `/antigravity.usage` still works for checking a provider that is not the active one.

Nothing about this needs new capabilities. Login already runs through `oauth`, `net`, and `credentials`, and usage reporting is ordinarily just another authenticated request. What it needs is a place for these three operations to live that the host can call generically, rather than each extension author inventing their own command name and the host having no way to know which command means what.

## Decision

Add `login`, `logout`, and `usage` as defined, optional exports on the `provider` world. An extension that does not support one returns a defined "not supported" result rather than omitting the function.

The host builds two things from these exports with no extra work by the extension author. A generic top-level `/login` lists every installed provider by name and calls the chosen one's `login` export; `/logout` and `/usage` behave the same way against the active provider. And the host automatically namespaces the same three exports under the extension's own name, so `/antigravity.usage` and `/codex.usage` exist without either author writing a prefix, and without risk of two providers colliding on a command both happened to call `usage`.

This lands before the ABI freeze in Phase 8, alongside the other pre-freeze punch list items. It is technically a breaking change to the `provider` world under the versioning policy's table, made deliberately before the freeze when a rebuild costs nothing; the policy's optional-export rule now states the constraint this pattern carries.

## Alternatives considered

Leave login, logout, and usage as ordinary commands each provider extension registers under its own name. This is what the `command` world already supports and needs no ABI change at all. It also means the host cannot build a generic `/login` picker or an aliased `/usage`, because it has no way to know that `/antigravity.login` and `/codex.login` mean the same kind of thing rather than two unrelated commands that happen to share a naming convention. Rejected because the whole value of the feature is the generic dispatch, which requires the host to recognize the operation, not just the name.

A separate `identity` world, distinct from `provider`, carrying only login, logout, and usage, that a provider extension additionally implements. This would let a non-provider extension also expose identity-shaped operations, which is not a case that has come up. It also splits one coherent concept, a provider's account state, across two worlds for no present benefit. Rejected as unneeded indirection until a real case for a non-provider identity surface appears.

## Consequences

The `provider` world's WIT surface grows by three functions, all optional in the sense that a provider without an OAuth login or a meaningful usage endpoint returns "not supported" rather than needing a stub that does nothing. Existing provider extensions built before this change need a rebuild to satisfy the expanded world and to gain the generic dispatch behavior. Before the freeze this costs a rebuild and nothing more; after 1.0, adding a function to a world is breaking like any other, and an optional function added then would need its own opt-in world per the versioning policy.

The command auto-namespacing needs a defined rule for what happens when an extension's own name would collide with a built-in command namespace. The host resolves this by reserving no such collisions in practice, since extension names are validated as a distinct identifier space from built-in command names, but the rule should be stated explicitly rather than left implicit.

The conformance extension gains cases for all three exports, including the not-supported path, since a host that mishandles a provider declining to support `usage` should fail a test rather than fail at a user's terminal.

## Revisit conditions

A case for other well-known provider-level operations beyond these three, which would argue for growing this set deliberately the same way the capability catalog grows, rather than for a different mechanism.
