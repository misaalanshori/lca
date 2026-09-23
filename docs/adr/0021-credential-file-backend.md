# ADR-0021: The credential backend for 1.0 is a file, not a keychain

Status: accepted.

Date: 2026-09-23.

## Context

The capability catalog says credentials prefer the platform keychain "where one exists". A Linux keychain needs a Secret Service/dbus dependency and macOS needs Security-framework FFI; neither is in the SRDD's closed dependency list, and writing the FFI by hand is exactly the kind of code that should not ship next to a credential store without a review the project has not budgeted for. NFR-14's owner-only file permissions are already implemented and tested.

## Decision

1.0 ships the owner-only file backend: one file per extension namespace under the agent's state directory, written with owner-only permissions (NFR-14, FR-PERM-6/7 isolation), inside the state directory that every `fs` scope refuses by construction. The keychain preference is deferred, not dropped: it becomes an ADR (0022 or later) with a written dependency justification — what it does, why hand-writing it is worse, its transitive footprint — before any such dependency is added.

## Alternatives considered

Add a keychain crate now, since the catalog prefers one. Rejected for 1.0: the closed dependency list is normative, the file backend already satisfies every functional requirement (isolation, permissions, refusal of the state directory to extensions), and a keychain's value is defense against an attacker who can already read the user's files but not their keychain daemon — real, but not the threat this release's threat model puts first. Written down so the deferral cannot quietly become "never".

## Consequences

- Credential storage works uniformly on all six targets with one tested code path.
- A user who wants keychain storage has to wait for the justification and the ADR; the phase log records this as an open decision, not a completed one.
- The `credentials` capability's surface does not change when the backend moves: extensions already speak only namespace-keyed get/set/delete.

## Revisit conditions

Any first-party provider that cannot store a secret it needs in a file (a hardware-bound credential, a vendor policy), or a user-reported threat-model gap where file storage of a token is the blocking issue.
