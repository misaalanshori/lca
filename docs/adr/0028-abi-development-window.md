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
