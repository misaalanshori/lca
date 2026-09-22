# Phase log

One entry per phase exit attempt: what ran, what passed, what blocked.

## Phase 0 — measurement and spikes: PASS

Exit test: "the size, startup, and streaming numbers exist and the team has
picked the default backend. NFR-1 and NFR-2 get their final values here."

Evidence: `docs/phase0-report.md`. Cranelift default backend confirmed
(ADR-0001 accepted), NFR-1 fixed at25 MB, NFR-2 at12 MB, streaming spike
measured (0.43 us/event), wasmi rejected with a receipt, and the
six-target cross-compilation matrix completed (all six build).

## Phase 1 — the core agent with no extensions: exit test INCOMPLETE

Exit test: "a user can hold a working coding session on Linux, macOS, and
Windows, resume it the next day, and the test suite passes on all three in
the pipeline."

Local evidence (Linux, x86-64):

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps`,
  and `cargo nextest run --workspace` (126 tests,0 failures) all green.
- The end-to-end suite drives the real binary against a loopback mock
  provider: headless `-p` turns, the full `--json` envelope contract, the
  exit-code table (0/2/3/4/5/6), session persistence with `resume` and
  `rename`, `config` sources, and `--version`. Interactive mode is covered
  by rendering and key-handling tests over ratatui's virtual buffer
  (streaming, resize,80 columns, plain color, the permission modal with the
  exact command).
- Release gates measured: binary5,667,320 bytes, cold start3.0 ms
  (`scripts/perf-gate.sh` passes).

Blocker (environment, not design): the repository's git credential is a
fine-grained personal access token **without the Workflows permission**.
GitHub rejects any push or contents-API call that touches
`.github/workflows/` (`refusing to allow a Personal Access Token to create
or update workflow ... without workflow scope`; the device-flow refresh
needs a browser this environment does not have). The complete three-OS
workflow is therefore parked at `ci/pending-workflows/ci.yml` with
activation steps in its README.

Consequences, stated plainly:

- The macOS and Windows legs of the exit test have **not** run. The suite
  has only executed on Linux x86-64. `cfg(windows)` code (Job Object
  process-tree kills) has been cross-compiled for review but never
  executed here.
- Phase 1's exit test is **not declared passed**. It passes when the
  workflow is activated and the three-OS pipeline is green.

Options, in order:

1. Grant the credential the Workflows permission (or push
   `.github/workflows/ci.yml` from an account that has it), activate the
   parked workflow, and let the pipeline run on all three OSes. This is
   the intended path and needs one human step.
2. If macOS/Windows runners remain unavailable, treat local cross-compile
   checks of the workspace as partial evidence only, and record the
   deviation in this log before any later phase claims to depend on it.

Subsequent phases continue building against the Linux-green suite; no
later phase may claim the Phase 1 exit test passed until option1 or2 is
resolved above.
