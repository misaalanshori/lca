# Architecture decision records

Version 0.1, 2026-09-20.

This directory holds the decisions that shape LCA. The software requirements and design document states what the system does. These records state why it does it that way, and what was rejected.

## Why these exist separately

A requirements document ages badly when it carries its own reasoning. Someone reads a design choice two years later, disagrees, and has no way to tell whether the alternative was considered and rejected or never considered at all. An ADR answers that question in one page.

A record is never edited to change its decision. When a decision changes, a new record supersedes the old one, and the old one gets a status line pointing at its replacement. The history stays readable.

## Format

Each record has a number, a title, a status, a date, and five sections: context, decision, alternatives considered, consequences, and revisit conditions. The revisit section matters more than it looks. It names the evidence that would overturn the decision, so a future reader knows whether new information counts as new information.

Status is one of: proposed, accepted, superseded by ADR-NNNN, or deprecated.

## Index

| Number | Title | Status | Phase that owns it |
|---|---|---|---|
| 0001 | WebAssembly runtime selection | Proposed | Phase 0 |
| 0002 | Crate decomposition | Accepted | Phase 1 |
| 0003 | Declarative widget tree for extension rendering | Accepted | Phase 6 |
| 0004 | Typed event stream for the provider world | Accepted | Phase 3 |
| 0005 | Named filesystem scopes | Accepted | Phase 2 |
| 0006 | Split permission store | Accepted | Phase 1 |
| 0007 | No out-of-process extension runner | Accepted | Phase 3 |
| 0008 | Build-time composition for extension dependencies | Accepted | After 1.0 |
| 0009 | Extension update path across ABI versions | Accepted | Phase 5 |
| 0010 | Distribution beyond OCI registries | Accepted | Phase 5 |
| 0011 | Local network access as a separate capability | Accepted | Phase 3 |
| 0012 | Provider world gains login, logout, and usage | Accepted | Phase 3 |
| 0013 | Three kinds of pluggability | Accepted | Phase 1 |
| 0014 | Async execution model | Accepted | Phase 1 |
| 0015 | Compaction and context transform as separate worlds | Accepted | Phase 4 |
| 0016 | A pty capability for interactive terminal sessions | Accepted | Phase 2 |
| 0017 | Prompt cache preservation and measurement | Accepted | Phase 3 |
| 0018 | Web-embedded extension hosting through sibling instantiation | Accepted | Phase 7 |
| 0019 | One dispatch trait for both delivery modes | Accepted | Phase 2 |
| 0021 | The credential backend for 1.0 is a file, not a keychain | Accepted | Phase 3 |
| 0022 | Ad hoc `net` grants live in the user grant store | Accepted | Phase 3 |
| 0023 | The OpenAI-compatible provider sends `x-opencode-session` | Accepted | Phase 3 |
| 0024 | The `/model` and `/compact` slots are host-side | Accepted | Phase 4 |
| 0025 | Pin the checked address for `net` connections | Accepted | Phase 3 |
| 0026 | Capability interfaces link in a denied state | Accepted | Post-release review |
| 0027 | Unsigned release artifacts, provenance-attested | Accepted | Post-release review |
| 0028 | The ABI development window — unfrozen now, frozen for good later | Accepted | Post-release review |
| 0029 | Typed image and multi-part content | Accepted | Cycle 2 |
| 0030 | Three bags — extension `resources`, `state`, and `credentials` | Accepted | Cycle 4 planning |
| 0031 | The `provider` world gains a login surface; presets are extension data | Accepted | Cycle 4 planning |
| 0032 | Embedded extensions serve their resources from the binary | Accepted | Cycle 4 planning |
| 0033 | The provider world's login surface | Accepted | Cycle 4 |
| 0034 | Skills handling is a host-side merge | Accepted | Cycle 6 |
| 0035 | `list-models` takes the settings `complete` does | Accepted | Cycle 6 |
| 0029 | Typed image content for provider messages | Accepted | Post-release review (cycle 2) |

ADR-0001 is proposed rather than accepted because Phase 0 measures the numbers that justify it. Every other record can be accepted on reasoning alone.

## Writing a new record

Copy the structure from any existing record. Keep it to one page. A record that needs more than a page is usually two decisions.

A pull request that changes an interface, a dependency with weight, a storage format, or a trust boundary needs a record. A pull request that implements an existing decision does not.
