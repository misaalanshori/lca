# ADR-0030: Three bags — extension `resources`, `state`, and `credentials`

Status: accepted.

Date: 2026-09-26.

## Context

An extension needs three kinds of data and had at most one clean place to
put each: package content (presets, `SKILL.md` documentation, templates,
arbitrary payloads), mutable persistence (caches, last-used values,
counters), and secrets (API keys, tokens). Today secrets have the
`credentials` capability, package content has nowhere to live, and the only
mutable path is the `private` fs scope — which drags in the whole `fs`
capability for "remember the last model used."

The owner's stance settles the shape: **the sandbox gates capabilities, not
creativity.** A package may carry any bytes it likes — presets, skills, a
WAD file rendered through the widget capability, model weights feeding a
pure-WASM inference extension — and only its *effects* are governed. The
extension structure must stay boring and the flexibility must come from
composition.

## Decision

Three bags, all keyed by **extension identity — never guest input**, so a
cross-extension read has no address to take at any privilege level (the
FR-PERM-6/7 guarantee):

| Bag | Mutability | Access | Grade |
|---|---|---|---|
| `resources/` | read-only (package content, versioned, replaced on update) | `resource-list` / `resource-read`, own tree only, no traversal, per-call size cap | data |
| `state/` | rw | `state-read/write/delete/list`, own namespace, size-capped, shown in `ext info`, `ext state clear`, wiped on uninstall | data |
| `credentials` | rw | the existing consented capability | secret (owner-only, never logged, never exported) |

The design bar for anything future: **if a use is expressible as a
composition of granted capabilities, it must be possible; if it is not, the
fix is a new capability or a new optional world — never a special case.**
Data conventions (resource kinds like `skills`, `provider-presets`) are
preferred over new imports; imports are added only when something truly
needs them (ADR-0028's bar).

Manifest `resources = [...]` declares kinds; the installer refuses
undeclared kinds and shows counts in consent (same declared-vs-shipped
strictness as `worlds`). Size caps are manifest-declared budgets (DoS
guards, not philosophy). User-visible configuration stays in host config
where the user can see it; `state` is opaque extension-internal data.

## Alternatives considered

- **Everything through the `fs` capability.** Too heavy and too wide: a
  presets file would require granting filesystem reach. Rejected.
- **Writable resources.** Breaks package immutability and update semantics;
  mutation belongs to `state`. Rejected.
- **Non-secret state in `credentials`.** Degrades the secrets promise
  (export, wipe, ACL rules) and teaches extensions to put junk in the
  secret store. Rejected.
- **A general `kv` without namespace isolation.** Cross-extension reads
  become thinkable. Rejected — identity-derived namespaces are the point.

## Consequences

ABI additions on the 0.2 line: `resource-list`/`resource-read` and
`state-*`, both delivery modes, conformance cases (round-trip, traversal,
cross-namespace, oversize) in the same change, `wit/CHANGELOG.md` lines.
Threat model gains its rows: skill text is prompt injection with a
distribution channel (consent + attribution + disable), preset-shaped data
can phish (masked prompt, per-extension credentials, the host-visible net
grant), resource bloat (declared budgets). `extension-authoring.md` gains
the conventions. Extensions reading their own data costs the host nothing
it did not already enforce at the same boundary.
