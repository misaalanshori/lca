# ADR-0048 — A capability-gated environment grant

Status: accepted, 2026-10-22 (gh #170). New capability; rides the 0.6 line's
in-place window (ADR-0028), no ABI label bump.

## The problem

A sandboxed WASM extension is instantiated with an **empty** `WasiCtx`, so
`std::env::var` inside a component always answers `NotPresent`. A native-linked
extension reads the process environment directly. The same provider therefore
worked compiled-in and broke as a `.wasm` — not because it used a capability
wrongly, but because there was no capability for the thing it needed.

The workaround history is the evidence that this mattered: `lca-cli` grew
`openai_ad_hoc_host()`, a hardcoded special case for one provider's base-URL
variable, because a provider extension had no legitimate way to read it.

## The decision

**A capability that forwards named host variables into the extension's sandbox at
instantiation.** The manifest lists exact names under `capabilities.env.keys`;
the host copies the values present on the host into the `WasiCtx` it builds.

Four properties, each chosen against a specific wrong answer:

1. **Exact names, no wildcards.** The names *are* the consent. "Read
   `DEEPSEEK_API_KEY` from your environment" is a sentence the user can evaluate;
   "read any variable" is not a consent screen at all, it is a blank cheque.
   This is the reasoning that already rejected a bare `*` under `net`
   (FR-PERM-15), applied where the stakes are higher, because an environment
   variable is often a secret rather than a destination.
2. **Unset is absent, never empty.** A declared variable the host does not have
   is omitted from the context. A set-but-empty variable is present and reads
   empty. Absence and emptiness are different facts, and an empty API key that
   looks present is how a provider sends a blank credential instead of failing
   honestly.
3. **Declared is not granted.** The manifest declares, the user approves. A
   declared name outside the approved set is absent. This is not new policy; it
   is what every capability already promises, restated because an env grant is
   the easiest one to implement in a way that skips it.
4. **Never the ambient environment.** Not even `PATH`. Forwarding the whole
   environment would be an information leak *and* a native/WASM mode difference
   nobody would predict — the exact class of defect this ADR exists to remove
   would reappear one layer down.

## Why no new host import

`wasi:cli/environment` already exists and already answers `NotPresent` for
anything the `WasiCtx` does not hold. Forwarding declared keys into the context
therefore needs **no WIT change at all**: the grant is enforced by construction,
at the moment the store is built, rather than by a check in a host function.

That is strictly better than a bespoke `lca:host/env` interface on three counts.
It is a smaller surface (no new WIT, no new import table entry, no new denial
path to test). It cannot be reached around (a guest cannot ask the context for a
variable the context does not contain). And it makes the native/WASM comparison
honest: both modes answer the same `std::env::var` call, so the test compares
*grants*, not two different implementations.

## Alternatives considered

**Inherit the host environment into every `WasiCtx`.** Simplest possible
implementation, and wrong: it hands every extension every secret the agent
process holds, with no consent and no record. Rejected outright.

**A `lca:host/env` import with a `get(name)` function.** The obvious capability
shape, and it buys nothing here that WASI does not already provide, while adding
an interface to version and a denial path to keep in step with the grant. If a
future need arises that `wasi:cli/environment` genuinely cannot serve (for
example, per-call values that change without re-instantiation), that is the time
to add the import — with its own ADR.

**A wildcard or prefix grant (`FOO_*`).** Rejected: see property 1. A prefix
grant makes the manifest shorter and the consent screen meaningless.

**Inferring the grant from what the extension already asked for** (for example,
scanning its code for `std::env::var` literals). Rejected: an inferred grant is
not a declared one, so the user would approve a set nobody wrote down, and the
manifest would stop being the record of what an extension may do.

## Consequences

- A provider can now be sandboxed and still authenticate from the environment its
  own documentation tells users to set, which is the point.
- **A native twin that reads `std::env::var` directly is still un-gated by the
  host** — it has no `WasiCtx` to forward into. The conformance twin applies the
  same declared list itself so the two delivery modes agree, and that is a
  property of this package's probe rather than something the host can enforce.
  Recorded here rather than left implicit: "compiled-in" must never become a way
  to read a variable the sandbox refuses. A native extension author who reads an
  undeclared variable gets no error — the host cannot see the read — which is why
  the declared list is the contract and the rows are the guard.
- The value is a **granted disclosure**, so the `credentials` secrecy rule
  applies: names may be shown, values never. No session record, no export, no
  diagnostic line, no `ext info` field carries a value.
- `lca-cli`'s `openai_ad_hoc_host()` workaround becomes redundant. It is **not**
  removed in this change: removing it is a separate risk with its own blast
  radius, and the workaround is harmless while it remains. Filed as follow-up.
- The manifest schema gains `capabilities.env`, which under the 0.6 line's
  in-place window (ADR-0028) needs no label bump: `lca 0.6.x` still ships
  `abi 0.6`, and an old manifest without the key parses unchanged.

## Guards

`tests/regressions/170-env-capability.rs` pins the parse rules (exact names
accepted, wildcard and `NAME=value` refused, `keys = []` a declaration),
declared-and-set, declared-and-unset (absent, not empty), set-but-empty (present
and empty), declared-not-approved (absent), the ambient environment never
forwarded (`PATH`/`HOME` excluded), and a manifest without the capability
resolving to an empty environment. The conformance extension carries
`env-get` in both delivery modes, so `conformance_diff`'s matrix compares the
granted / declared-but-unset / undeclared answers byte for byte.
