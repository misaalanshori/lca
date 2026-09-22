# Phase 0 report: measurement and spikes

Date: 2026-09-22. Targets measured: `x86_64-unknown-linux-gnu` host, Rust 1.98.1,
Wasmtime 49.0.0, wit-bindgen 0.41.0. Machine: 4 cores, 8 GB RAM class.

This report is the Phase 0 exit-test evidence required by `docs/lca-srdd.md`:
the size, startup, and streaming numbers exist, and the default backend is
picked. The spike lives under `phase0/`: a guest component implementing a
two-function world (`compute` plus a pull-based `events` resource exporting
10,000 events) and a host that instantiates it through `wasmtime::component::bindgen!`,
mirroring the shape ADR-0004 and ADR-0014 settle on (pull resource, host-driven
polling, same-process Tokio-less synchronous store for the spike).

## What was validated

- The Component Model toolchain works end to end on stable Rust: guest built
  with `wit-bindgen` to `wasm32-wasip2` (74,679-byte component), host typed
  bindings generated from the same WIT. The provider-world streaming spike
  (resource with `next`, host pulls) works and is fast (tables below). The
  risk-table row "streaming across the component boundary is slow or awkward"
  is resolved: keep the pull resource, no need to invert the flow.
- Cross-target precompilation works: the Cranelift host precompiled the
  component to Pulley bytecode (`Config::target("pulley64")`, 255,968-byte
  artifact), and the interpreter-only host (no Cranelift feature) deserialized
  and ran it. Precompile-per-digest (ADR-0001) is viable; one-time cost below.
- `wasmi` 2.0.0 cannot load components: it rejects our component with
  `encoded as a component but the WebAssembly component model feature is not
  enabled`. wasmi exposes no Component Model feature, so it is not a drop-in
  fallback for the typed ABI. The SRDD fallback "wasmi for an interpreter-only
  build" therefore means writing core-module marshaling glue (the risk table's
  last resort), not a feature switch. Wasmtime Pulley covers the
  interpreter-only need instead:4.97 MB with no compiler code at all.

## Binary size (NFR-1, NFR-2)

| Build | Cargo features | Stripped size |
|---|---|---|
| Host, Cranelift + Pulley (default) | `cranelift` | **17,276,120 B (16.5 MiB)** |
| Host, Pulley interpreter only | `pulley`, no compiler | **4,973,432 B (4.7 MiB)** |

Sizes are for the spike host, which links the full Wasmtime runtime, the
component model, and `wasmtime-wasi`. The agent adds tokio, ratatui, hyper,
serde, clap, and the session/tool crates on top; Phase 1 measures the real
binary and Phase 7 (release gate) re-checks the same thresholds.

**Fixed threshold values (this phase owns them):**

- NFR-1: default native binary ≤ **25 MB** on x86-64 Linux. Provisional value
  confirmed as the shipped threshold: the runtime alone measures16.5 MiB, and
  the remaining ~8 MB covers the agent's non-Wasmtime dependencies with
  `lto = "fat"` and `strip` on. Ratchet down per the release policy if Phase 1
  comes in well under.
- NFR-2: interpreter-only build ≤ **12 MB** on x86-64 Linux. Provisional value
  confirmed: the Pulley-only runtime measures4.7 MiB, leaving >7 MB of
  headroom.

## Startup and instantiation (NFR-3, NFR-4)

End-to-end process wall time,40 runs each, measured from `fork+exec` of the
spike binary to exit, where each run instantiates the component, calls
`compute`, and drains all10,000 stream events:

| Scenario | median | min | max |
|---|---|---|---|
| Cranelift, precompiles on every start (no artifact cache) | 58.8 ms | 47.9 ms | 90.5 ms |
| Cranelift, cached precompiled artifact (steady state, ADR-0001) | **10.4 ms** | 9.1 ms | 12.3 ms |
| Pulley-only, precompiled Pulley artifact | 23.0 ms | 20.2 ms | 26.7 ms |

Inside the process (median of30 runs):

| Metric | Cranelift | Pulley only |
|---|---|---|
| Engine construction |0.2 ms |0.1 ms |
| Deserialize precompiled component |0.65 ms |0.59 ms |
| Instantiate component + linker | **0.18–0.22 ms** | **0.19 ms** |
| Precompile (one time per digest) |41–55 ms | n/a (no compiler) |
| Serialized artifact size (this guest) |215,224 B (native) |255,968 B (pulley) |

NFR-4 (instantiation ≤20 ms) has ~100× headroom. NFR-3 (cold start to prompt
≤150 ms on a 2020-class laptop, no extensions): the spike's steady-state
10.4 ms plus the Phase 1 TUI's own startup leaves comfortable room; Phase 1
measures the real binary and fixes nothing here.

## Streaming and hook overhead (NFR-5, per-event cost)

| Metric | Cranelift | Pulley only |
|---|---|---|
|10,000-event pull stream, median total |4.25 ms |17.9 ms |
| Per event | **0.43 µs** |1.79 µs |
| Single `compute` call round trip, median | **0.23 µs** |0.60 µs |

NFR-5 (≤1 ms hook overhead above the extension's own work) has four orders of
magnitude of headroom on Cranelift and still ~1600× on Pulley. One design
conclusion: a tool-call stream of realistic size (hundreds of events) costs
well under a millisecond of boundary crossing, so per-event rendering
decisions, not ABI cost, will dominate.

## Cross-compilation matrix (six native targets)

`phase0/matrix.sh` builds the spike host for every release target and records
the result. Raw log: `phase0/matrix.log` (gitignored; summary here).

MATRIX_RESULTS_PLACEHOLDER

## Backend decision

**Default backend: Wasmtime with the Cranelift backend. ADR-0001 is confirmed
and moves to accepted.** Evidence:

- Size:16.5 MiB stripped for the full runtime with Cranelift, inside the
 25 MB default-binary budget even before LTO.
- The alternative that the risk table names, wasmi, cannot run the Component
  Model at all (receipt above), so "interpreter-only via wasmi" would cost the
  typed ABI or hand-written canonical-ABI glue; Wasmtime Pulley delivers the
  interpreter-only build instead at4.7 MiB, well inside the12 MB budget.
- Startup, instantiation, and per-call overhead are all far inside their
  thresholds on both backends.

Pulley remains the build for environments that forbid executable memory
(NFR-15): the `pulley` feature build contains no compiler and loads only
precompiled bytecode artifacts.

## Exit test

> the size, startup, and streaming numbers exist and the team has picked the
> default backend. NFR-1 and NFR-2 get their final values here.

Numbers: above. Backend: Wasmtime + Cranelift (Pulley for no-JIT targets).
NFR-1 fixed at25 MB, NFR-2 fixed at12 MB. **Phase 0 exit test passes.**
