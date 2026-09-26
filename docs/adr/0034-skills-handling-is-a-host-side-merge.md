# ADR-0034: skills handling is a host-side merge

Date: 2026-09-26. Status: accepted.

## Context

ADR-0013 classified skills handling as a runtime extension: it shipped as a
`context-transform` consumer under `extensions/skills/`, bundled and
enabled by default. That was the right shape when the only source of skill
text was the workspace.

Cycle 4 added ADR-0030's three-bag model, and skills became **extension
data**. A package can ship `resources/skills/<name>/SKILL.md`, and the host
reads that kind for prompt assembly (ADR-0030's "who reads what" table).
So the feature now has three sources with a precedence order:

1. the workspace's `.lca/skills` (project),
2. the user skills dir (`<config>/skills`),
3. every installed package's `resources/skills/<name>/`.

A `context-transform` extension cannot do this merge. It can only read its
*own* `resources` bag — that is the whole point of the bag's isolation
(FR-PERM-6/7: no cross-namespace read at any level) — so a skills extension
could never see another extension's skill pack. And even if it could, the
three sources need one precedence order and one attribution rule, which is
definitionally not something a chain of independent transforms can provide.

The alternative was to give the skills extension a new capability meaning
"read every extension's resources", which would punch a hole through the
bag's isolation for one feature. ADR-0030's design bar says the sandbox
gates capabilities, not creativity; a hole is a hole.

## Decision

The host owns the skills merge and injection. `crates/lca-core/src/skills.rs`
collects the three sources, resolves precedence (project > user >
extension, first name wins), and injects the result as an attributed
system message into `turn_body`, after the transform chain.

`extensions/skills` stays in the tree as a working `context-transform`
example and is **not registered**. The `bundled-skills` Cargo feature still
builds it; it is no longer a switch that changes what runs.

This is a category change under ADR-0013 (fixed core, build-time backend,
runtime extension): skills handling moved from *runtime extension* to the
host side of the divide. It is recorded here rather than silently edited
into ADR-0013, whose rows are annotated to point here.

## Consequences

- Precedence and attribution are real: one merge, one rule, and every
  injected skill names its source.
- A skill pack is now a **data-only package** (`worlds = []` plus a
  `resources` bag, ADR-0032), which installs and removes through the
  ordinary pipeline with no component at all.
- The `context-transform` world loses its only first-party consumer beyond
  compaction. Its proof stands (the skills extension did serve it), and the
  conformance extension still exercises the world in both delivery modes.
- A user who explicitly wants transform-chain behavior can still install
  the example; it just does not do the three-source merge.

## Alternatives considered

- **A `read-all-resources` capability.** Gives the extension the reach it
  needs and breaks the bag's isolation for every other extension. Rejected:
  the isolation is the security property, not an obstacle.
- **Host reads the bags, extension merges.** The host would have to hand
  every extension's bag to the skills extension anyway, which is the same
  hole with an extra step. Rejected.
- **Keep the merge in the extension and only support one source.** Drops
  the user and package sources that ADR-0030 exists to provide. Rejected.
