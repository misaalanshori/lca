# ADR-0022: Ad hoc `net` grants live in the user grant store

Status: accepted.

Date: 2026-09-23.

## Context

`docs/configuration.md` places extension enablement, project trust, and ad hoc grants in the user grant store, keyed by the canonical project path — but the grant store had no place to put an ad hoc `net` pattern, and the capability engine's grants were built from the manifest alone. Without persistence, the ad hoc attach at the moment a base URL is named (FR-PERM-16) would work for one process and forget everything afterwards, and the OpenAI-compatible provider pointed at a custom endpoint would deny every request on the next run.

## Decision

The grant store's per-project entry gains one additive field, `net_patterns`, a set of host-pattern strings in the `net` vocabulary, written through `approve_net_pattern` (which validates the pattern first) and read back through `net_patterns`, which skips anything that no longer parses rather than failing every later request over one corrupt line. The provider wiring merges a project's stored patterns into the engine's `adhoc_net` grants when it builds the capability environment for the bundled provider. The field defaults on read, so every existing store file still loads.

## Alternatives considered

Reuse the existing `patterns` set. Rejected: that set holds the `fs`/prompt vocabulary (paths and scope words), and the `net` vocabulary is hostnames and `host:port` pins — single-label names are refused on both sides precisely so the two vocabularies cannot be confused, so sharing one set would make every future reader guess which dialect a string belongs to.

Keep ad hoc grants only in memory for the session. Rejected: FR-PERM-16 describes attaching the grant when the host is named; a grant that evaporates turns every subsequent run into a denial the user already consented to, which trains people to route around the prompt.

## Consequences

- An approved ad hoc host survives restarts, per project, and never leaks across projects (tested).
- The consent UI that calls `approve_net_pattern` arrives with the install/login flows (Phases 5–7); until then the only writer is those flows and the tests that stand in for them, and a custom base URL set purely through the environment stays denied until the modal exists — recorded in the phase log rather than papered over with an automatic grant.
- The grant store's format grows one defaulted field; nothing else reads it.

## Revisit conditions

If ad hoc grants ever need port pins with expiry, per-extension scoping, or an audit trail of who attached them, this field becomes a table and the storage format gets its own record then.
