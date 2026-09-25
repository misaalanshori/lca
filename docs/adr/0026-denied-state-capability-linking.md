# ADR-0026: Capability interfaces link in a denied state

Status: accepted.

Date: 2026-09-25.

## Context

The capability catalog originally specified two failure shapes: an interface a
world does not import fails at link time, and an interface that exists but was
never granted *also* failed at link time, because the host was supposed to
build the import table from the granted set. The implementation does something
different: the host links every capability interface a world carries, all in a
denied state, and refuses an ungranted call at the call boundary with a
recorded permission error (FR-PERM-3). The post-release review found the
mismatch (`../issues-20260925-1321.md`, F1, against prior finding #17) and the
question was whether to move the code to the doc or the doc to the code.

The runtime model is what makes FR-PERM-18 work as specified: an ad hoc grant
takes effect for subsequent calls *without re-instantiation*, and a revocation
takes effect at the next instantiation. A granted-set import table is fixed at
link time; changing a grant would mean relinking or re-instantiating the
component. The denied-state model absorbs both changes as state, not as
relinking.

## Decision

Denied-state linking is the model, and `docs/capabilities.md` is normative.

The host links every capability interface the component's world imports, all
denied by default. A call on a capability that was never granted, or whose
parameters a grant does not cover, returns a permission error the extension can
handle, and the denial is recorded with the extension identity, the capability,
the attempted parameter, and the time. An interface the world does not import
at all is absent from the link, so manifest/code mismatches still fail loudly
at load. Grants, ad hoc grants, and revocations flip the per-call check; they
never touch the link.

## Alternatives considered

- **Granted-set import table (the original doc).** The strongest "an ungranted
  capability cannot even be called" property. Rejected: grants that change
  mid-session (FR-PERM-18) would need relinking or re-instantiation to take
  effect; an extension that can gracefully report "this feature is not
  permitted" instead of failing to load is friendlier to optional
  capabilities; and the mismatch-detection property is preserved at the world
  level either way.
- **Denied-state linking with a strict mode that removes ungranted interfaces
  at link time.** Two behaviors to test and document for a property the
  runtime check already enforces. Rejected for 1.0; revisit if an audit shows
  host call sites that forget the check.

## Consequences

The per-call denial check is the single enforcement point, so it is the thing
to fuzz and review (`fuzz/corpus/abi_decode`, the conformance extension's
permission cases, and the threat model's extension scenarios all exercise it).
Denial records stay meaningful (`lca ext info <name>` shows attempted-but-
denied calls, which is the user-visible signal that an extension is probing).
The catalog's "deny by default" wording means denied-at-the-boundary, not
absent-from-the-link, and the threat model's extension scenarios match that
shape.
