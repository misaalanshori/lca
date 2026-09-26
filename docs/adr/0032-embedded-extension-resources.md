# ADR-0032: Embedded extensions serve their resources from the binary

Status: accepted.

Date: 2026-09-26.

## Context

LCA ships as a single static binary, and first-party extensions like
`openai-compatible` are compiled into it (ADR-0013's build-time category).
ADR-0030 gives extensions a `resources/` bag — which the compiled-in
openai-compatible needs for its login presets. Shipping those as sidecar
files beside the binary breaks the single-static-binary property; duplicating
the data (one copy embedded, one packaged) drifts; extracting to disk on
first run adds IO and cache-coherence questions for a few kilobytes.

## Decision

Native-compiled extensions **embed their resources at build time** (a
`include_bytes!`-backed static table), and the capability engine's resource
seam serves them exactly as it serves installed package files: the seam is
engine-level, not filesystem-level, so `resource-list`/`resource-read`
returns identical bytes and identical errors in both delivery modes. The
conformance diff (native vs WASM) proves it, as it proves every other host
import.

**One source file, two deliveries:** `extensions/<name>/resources/**` is the
single source of truth — the component build packages it, the native build
embeds it. A build check asserts the two copies cannot drift (hash
comparison in the existing gate style).

Size is trivial (presets are KBs; the size-and-startup gate already budgets
the binary), but embedded resources count against the gate like any other
bytes. `state` and `credentials` for embedded extensions use the same
identity-derived namespaces as everyone else (real directories under the
state directory).

## Alternatives considered

- **Sidecar files next to the binary.** Violates single-static-binary, the
  distribution story, and the reproducible-build story. Rejected.
- **Extract embedded resources to the state dir on first run.** IO, stale
  copies across upgrades, and permissions work to serve bytes that could
  have been served from memory. Rejected.
- **Two hand-maintained copies.** Drift is a matter of time. Rejected.

## Consequences

The binary carries first-party data inline (KBs, watched by the size gate).
Built-in extensions are **disable-only** — you cannot uninstall what is
compiled in; removing the files is a rebuild. That policy line joins the
extension docs. The resource seam's contract ("your own bag, no traversal,
no cross-extension reach") is identical for both delivery modes, which is
the NFR-25 property the whole extension model promises. An installed
third-party extension and a compiled-in first-party one differ only in
where the bytes were found.
