# ADR-0028: The ABI development window — unfrozen now, frozen for good later

Status: accepted.

Date: 2026-09-25.

## Context

Phase 8 froze `lca:ext` at 1.0 (executed 2026-09-23). The freeze bought real
discipline: `ADR-0024` kept `/model` and `/compact` host-side, the login flow
was built host-side rather than growing `lca:host/ui`, and the capability model
found its denied-state shape (ADR-0026). Those wins are kept.

The timing was wrong. The freeze protects a published third-party ecosystem
from churn; there is no third-party ecosystem yet, while the interface is
still discovering what it needs to be. The costs are visible already: ADR-0024
had to refuse a command export on freeze grounds, the B2 login work was
constrained to "no ABI change" paths, and the image question (B3/D3) forces a
structural feature through the untyped `extras` side channel or into a far
future 2.0. The project is a 0.1.x product carrying a 1.0-stable interface it
cannot yet honestly promise.

The owner's decision: a **development window** — temporarily unfrozen, but not
casual — with the freeze returning once the ABI has been stressed by real
features and judged mature.

## Decision

**Phase A, the development window (now).** `lca:ext` returns to the 0.x
line, where the minor position is the breaking position
(`docs/abi-versioning.md` § "Pre-1.0 reality" becomes the active law).
Breaking changes are permitted, and everything around them stays mandatory:

- The bar for a change is **"this interface is truly needed"** — no
  speculative surface, nothing added for completeness. A string vocabulary or
  an `extras` entry is preferred over new structure unless the structure is
  load-bearing.
- Every change moves through the full machinery: WIT changelog entry, minor
  bump (`0.2` → `0.3` …), the conformance extension updated in the same
  change, and the support window honored (current + previous minor), so an
  extension author is never whiplashed without a rebuild cycle's notice.
- A change that is a real interface decision gets an ADR. The typed image and
  multi-part content is the window's first such decision.

**Phase B, the re-freeze — for good.** The freeze returns when the owner
judges the ABI mature after the planned feature work lands, evidenced by the
criteria below. Then `docs/abi-versioning.md`'s post-freeze law returns
verbatim: breaking is a major version, weighed heavily, infrequent, and
justified only by "the project cannot ship without it." The standing note:
**the ABI must eventually be locked, so third-party extensions are never
constantly broken.** This ADR defers the freeze; it does not cancel it.

Criteria for the re-freeze (the judgment is the owner's; these are what the
judgment should rest on):

1. The typed content and any other window changes have shipped and survived
   contact with both bundled providers.
2. A second host implementation exercises the same ABI (the web/embed work is
   the natural candidate — nothing finds an overfit interface like
   implementing it twice).
3. A stability window: consecutive releases with zero ABI changes.
4. No "this will have to change when X" notes left against the ABI in the
   deferred work plan.

## Alternatives considered

- **Permanently unfreeze.** Becomes Pi's situation: extensions break on a
  regular cadence and the compatibility promise never lands. Rejected — the
  freeze is the project's differentiator; only its timing was wrong.
- **Stay frozen.** Keeps paying the rigidity cost while the interface is still
  being discovered (typed features forced through `extras` or blocked). Rejected.
- **Keep the 1.0 label but allow breaking 1.x changes.** Lies about semver and
  makes the version number untrustworthy. Rejected.

## Consequences

The manifest line moves (`abi = "0.2"` on the first window change), WIT
package versions move with it, and fixtures rebuild — mechanical, once per
change. The support-window test (NFR-19) keeps its meaning; "previous minor"
now means "previous breaking line." The `extras` rule, the optional-world
rule, and the deprecation policy are untouched. `ADR-0024` is annotated: its
freeze-based argument softens under this record, while the host-side slot
decision stands on its own merits. `docs/abi-versioning.md`, the requirements
document's Phase 8 note, and the release policy carry the dated amendment;
the Phase 8 exit test recorded its pass as written on the day it passed.

## Annotation — 2026-09-27 (the versioning sync): the ABI line tracks the product minor

Owner decision, 2026-09-27. This supersedes the 2026-09-25 annotation's
"one line forever" shape and keeps everything else that annotation settled.
Both annotations stand as written; where they disagree on how the line
moves, this one governs.

**The rule.** On the 0.x line, the ABI line tracks the product minor:
`lca 0.x.y` ships `abi 0.x`. Every release train bumps both together — the
WIT package, every manifest, and the fixtures move at release cadence,
which is the mechanical bump the previous annotation avoided per change,
not the churn it was written against. Within a line the interface still
mutates **in place**: a breaking change does not bump the number. What is
dropped is per-change churn; what is kept is one honest label per train.

**What this buys.** The public story is one sentence — "0.x = unstable,
1.0 = locked." A stale extension gets a legible failure (built against
`abi 0.2`; this interface moved) instead of an invisible same-line
mismatch, and the current-plus-previous support window regains meaning
across trains. Same-line indistinguishability shrinks from "the whole
window" to "inside one train" — see the named limitation in
`docs/abi-versioning.md`.

**The endgame is unchanged.** `lca 1.0.0` ships `abi 1.0` — the joint-ship
rule stands; the freeze is a snapshot of the final 0.x line with zero
interface change; after it, majors stay joint and "breaking = major" is
true again as written.

**Transition (the one mismatch, documented).** `v0.3.0` shipped labeled
`abi 0.2` — the last release before this rule. The next release is `0.4.0`
carrying `abi 0.4`; the `list-models` signature change (ADR-0035) rides in
the 0.4 train. Published 0.2-line extension artifacts are stale against
that change and are rebuilt with 0.4.0. `0.3.0 / abi 0.2` is the transition
artifact, not a pattern.

## Annotation — 2026-09-25 (cycle 2): 0.2 is a single in-place development line

The decision above stands; this narrows how its window is versioned. It does
not change what a breaking change *is*, only whether the number moves for each
one.

**0.2 is one line.** Breaking changes land inside it without a minor bump: the
WIT package stays `@0.2.0`, every manifest stays `abi = "0.2"`, and
`wit/CHANGELOG.md` carries a running "0.2 development" section instead of a
per-minor entry. The line moves only if a checkpoint or an external extension
needs the signal (then `0.3`); it does not move per change. This is deliberate:
there is no published third-party ecosystem, so the per-change version churn
buys nothing and the interface is still being discovered.

**What does not drop.** Every breaking change still lands its conformance
update in the same change, and a real interface decision still gets an ADR.
The version churn is dropped, not the discipline.

**The support window narrows, knowingly.** Because same-line builds are
indistinguishable, the current-plus-previous guarantee protects cross-line
moves (0.1 → 0.2) but not same-line changes inside 0.2. NFR-19's test keeps
running and still checks the cross-line window; for 0.2.x it guarantees less.
`docs/abi-versioning.md` names this as a limitation.

**The refreeze is a snapshot-and-relabel to 1.0, not a new line.** Zero
interface bytes change between the final 0.2 and 1.0; 1.0 *is* 0.2's end
state, and 0.2 keeps loading on the 1.0 host as the boundary amnesty — the
same shape as the 0.1 amnesty at the 2026-09-23 freeze. This is why the
freeze label is 1.0 and not 0.3: under 0.3 the first post-freeze break would
be 0.4, a minor bump mechanically identical to the churn this annotation
removes, and the "breaking is heavy" signal would vanish exactly when it
matters. At 1.0 the post-freeze "breaking = major" rule becomes true again as
written, so no re-labelling is needed.
