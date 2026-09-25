# ADR-0027: Unsigned release artifacts, provenance-attested

Status: accepted.

Date: 2026-09-25.

## Context

The release policy as written required the macOS artifacts to be signed and
notarized and the Windows artifacts to be signed. Those credentials do not
exist in this project's build environment, and they cannot be created by
automation: an Apple Developer account with notarization and a Microsoft code
signing certificate are purchased identities. The choice was therefore to hold
releases until credentials exist, or to release unsigned and say so. The
post-release review flagged the policy text being softened in place
(`../issues-20260925-1321.md`, F2); this record ratifies the change.

Two existing properties absorb most of what signing would have carried: the
release build is reproducible (a tagged commit produces byte-identical
binaries, double-built and compared in the pipeline), and every artifact is
provenance-attested through GitHub's `actions/attest-build-provenance`, so a
verifier can check that a binary came from this repository at a known commit,
built by the published workflow. SHA-256 checksums ship alongside.

## Decision

0.x releases ship unsigned. Trust rests on reproducibility plus the provenance
attestation plus the checksums, in that order. `docs/release-policy.md`'s
artifact matrix says "Unsigned; provenance-attested" per target, and the
verification instructions are the attestation verification, not a signature
check.

Code signing and notarization return when the project holds signing
credentials; when that happens, the artifact matrix changes back and this
record is annotated superseded. Until then no release claims to be signed.

The `wasm32-wasip2` npm row of the artifact matrix stays deferred with NFR-11
(`scripts/deferred-requirements.txt`), unrelated to signing.

## Alternatives considered

- **Hold releases until signing credentials exist.** Blocks shipping on a
  purchase decision. Rejected.
- **Sign with a self-generated key.** No third-party identity attaches to it;
  the attestation already proves provenance better. Rejected.
- **Ad hoc macOS signing (identity "-").** Changes Gatekeeper's failure mode
  without improving trust. Rejected.

## Consequences

Users see OS-level warnings for unsigned downloads on macOS and Windows until
signing lands; the README and release notes state why and how to verify
instead. The provenance attestation becomes load-bearing, so the workflow that
produces it is part of the trusted computing base and its pinned actions are
reviewed like any other dependency. A future release policy edit may restore
signing rows; it does not require touching this record's text (annotate +
supersede per `docs/abi-versioning.md`'s amendment habit).
