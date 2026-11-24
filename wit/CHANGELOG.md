# Changelog

## 0.7 line

Three interface changes on this line: gh #581 and gh #352 add cases to exported
variants, and gh #407 (below) opened the line with a host-import addition.

### gh #352: `content-block` gains a typed `reasoning` case

A record and one case added to the shared content variant (ADR-0050):

```wit
record reasoning-block {
  text: string,
  signature: option<string>,
}

variant content-block {
  text(string),
  image(tuple<string, list<u8>>),
  reasoning(reasoning-block),
}
```

**Breaking for the contract, additive for old artifacts.** A guest that maps content
blocks gains an arm (the compiler names every site); a guest that only constructs
messages is untouched. The case has to carry a payload, which is why it is a record and
not the payload-free shape gh #581's truncation case has.

**`signature` is `option<string>` on purpose.** Absent and empty are different facts: a
vendor that issued no signature must send no field, and the gateway rejects both casings
of an empty one. The `text` half may be empty - some vendors redact the reasoning and
keep only the signature, the shape the native path already carried.

**The old channel still works for one minor version.** The host keeps writing the
reserved `thinking-signature` message extra, because a guest built against `@0.7.0`
before this case rebuilds its reasoning block from that key; dropping the write would
lose its signature silently, and a provider then fails its replay check. Converted
guests read the typed case and never the key, and
`tests/regressions/352-typed-reasoning-block.rs` scans every guest source to keep that
true. The write goes away after the first minor release that follows this case, when no
in-tree guest names the key.

**The wire rule that came with it:** on the outgoing Gemini part the signature is a
**sibling** of `functionCall`, never nested inside it - the gateway rejects the nested
placement live, in both casings, and the public docs suggest the opposite. That is the
extension's own serialization, not the ABI's, but it is the reason the block carries the
signature rather than a separate field, so it is recorded here and in ADR-0050.

**What an author must do:** rebuild against the 0.7 `wit/`. If your guest maps content
blocks, handle the `reasoning` case (a provider that wants replay fidelity returns the
block with its signature; a transform that ignores it can pass it through unchanged). If
your guest needs a replay signature, read it from the block - the extras key is on its
way out.

### gh #581: `stream-event` gains a typed `truncated` case

One case added to the `provider` world's exported event variant, carrying no
payload:

```wit
variant stream-event {
  // ... text-delta, reasoning-delta, tool-call-*, usage, error ...
  truncated,
  vendor-event(tuple<string, string>),
}
```

**Breaking for the contract, and this is the case the escape hatch was for.** A
guest *consumes* this variant only if it does something other than construct it;
such a guest now has a case it must name (or cover with `_`). A guest that
*constructs* events needs no arm to keep compiling, and one that wants to report
a cut emits `truncated` instead of `vendor-event("truncated", "")`. The host is
the only consumer in this tree, and it gained the case in the same change.

**The old shape still works for one minor version.** `vendor-event("truncated",
"")` keeps mapping to the truncated fact on the host, because a component built
against `@0.7.0` before this case landed - or a third-party guest copying the
round trip from an older doc - can still send it, and dropping the read would
turn its truncated answer into a complete one silently. The typed case is the
path every in-tree guest now takes; `TRUNCATED_KIND` is kept as that compatibility
read and goes away after the first minor release that follows this one, once no
in-tree guest emits it (`tests/regressions/581-typed-truncated-event.rs` is the
scan that makes the condition checkable).

**Why:** the turn depends on the truncation for a user-visible banner, and before
this case the fact crossed the boundary as a string kind plus an empty payload -
checked on both sides, but a typo in either spelling was a compile-time no-op and
a runtime loss. A variant case makes the compiler carry it.

**What an author must do:** rebuild against the 0.7 `wit/`. If your guest reports
a truncation, emit `truncated` (not the vendor event). If your guest matches on
`stream-event`, add the case or a `_` arm.

### gh #407: `ui-dialogs` gains the tabbed questionnaire

**This change opens the 0.7 line.** The 0.6 line was frozen for interface
changes on 2026-10-08 (`docs/abi-versioning.md`), and the freeze says the next
WIT addition opens the next line with its own annotation — so the package
version moves `0.6.0` → `0.7.0` and every manifest rebuilt against this `wit/`
declares `abi = "0.7"`. The label moves; the support window does not shrink:
0.6 components keep loading on a 0.7 host (current line plus the one before).

One function and two records added to a host import interface:

```wit
record question {
  header: string,
  prompt: string,
  options: list<string>,
  multiple: bool,
}

record answer {
  header: string,
  value: string,
  custom: bool,
}

questionnaire: func(title: string, questions: list<question>) -> result<option<list<answer>>, string>;
```

**Breaking for the contract, additive for old artifacts.** A guest rebuilt
against this `wit/` sees the new function and records on
`lca:host/ui-dialogs@0.7.0` — the interface it already imports — so a guest
that wants the questionnaire has to be rebuilt, and one that does not is
untouched. A component built *before* this line imports the same interface and
never calls the new member, which the host satisfies whether or not it is
called (the rule `tools` established in gh #77 and `net` repeated in gh #302);
`tests/regressions/407-questionnaire-abi.rs` loads such a component and drives
its tool, so the claim is proven rather than asserted. That argument is about
**link** compatibility, and it is why the *window* keeps 0.6 artifacts loading;
it is not what decides which line a change belongs to — the freeze does, and
the freeze is why this is 0.7 rather than a quiet addition to a released 0.6.

**Why:** today a model asking three clarifications costs three sequential host
pickers - `extensions/question`'s `ask_all` loops `select`/`input` once per
question - with no progress indicator, no per-question state, no review and no
single submit. With this member N questions are answered in **one** modal, one
turn, one submit: tabs with `■`/`□` answered/pending markers, a review frame
before it fires, and an inline "type your own" editor per tab. The chrome stays
the host's (an extension still never draws a modal, `docs/extension-authoring.md`
§dialogs), so the spoof-proofing does not change with the answer count.

**What an author must do:** rebuild against the new `wit/` and declare
`abi = "0.7"`. No other source change is needed for any world — every export in
every `world-*.wit` is byte-identical to 0.6's; the only interface change is the
one recorded here, and it is an addition to a host import. An extension that
declares `abi = "0.6"` keeps loading on a 0.7 host (the window) but cannot use
`questionnaire`.

## 0.6 development window — gh #302: the `tool` world imports `net`

One import added to the `tool` world:

```wit
import lca:host/net@0.6.0;
```

**Breaking for the contract, additive for old artifacts.** A world that gains
an import requires every component built against the new `wit/` to satisfy it,
so a rebuilt guest must map the interface (thirteen tool-world crates gained the
one `with:` line). But the host satisfies an import whether the guest calls it
or not — the rule established for `tools` in gh #77 — so a component built
*before* this line still links and runs. That is the claim "additive" makes, and
it is proven rather than asserted:
`tests/regressions/302-tool-world-net-import.rs` loads `tool-legacy`'s
committed component (built against a tool world with no network at all).

**Why:** `extensions/webfetch` had no way to reach the network in WASM mode, so
its tool existed only on the native path and returned a deferral in the
sandbox. With this import both halves run the same `fetch` → `convert`
pipeline over a `NetView`, which is what NFR-25 asks. The interactive per-host
consent behaviour landed earlier under the same issue and is unchanged: a
request from the `tool` world asks for an ungranted host (the model or user
chose that URL), every other world keeps the plain deny, and `[capabilities.net]`
is still required — the import grants nothing by itself.

**No label bump.** This is an in-place window change (ADR-0028); `lca 0.6.x`
ships `abi 0.6`.

## 0.6.0 (ABI train, 2026-10-07 — cycles 3+4: gh #172 + #124, gh #77 + #45)

The train's last break, then the freeze: the `tool-catalog` world
(multi-tool suites with exposure and namespaces), the `lca:host/tools`
import (nested calls, active-set control, callable listing), and eight
opt-in hooks worlds. The `tool` world and the six-point `hooks` world
are byte-identical: single-tool guests and old hook guests load and
run unchanged. Same label, same window (`0.5..=0.6`); after this
cycle `wit/` + `schemas/` freeze for the 0.6 line (the freeze note in
`docs/abi-versioning.md`).

- **Added:** the `tool-catalog` world: `get-tools` over `tool-spec`
  (`name`, `description`, `parameters`, `exposure`, `namespace`,
  `annotations`, `extras`) and `run-tool(name, call)` (gh #77).
  Exposure is a validated string (`direct`, `model-only`, `codemode`,
  `deferred`, `hidden`); only `direct` tools declare.
- **Added:** the `lca:host/tools` import (`execute-tool`, `list-tools`,
  `set-active-tools`, `get-active-tools`), imported by the `tool` and
  `tool-catalog` worlds and gated by the `tools` manifest capability
  (gh #77). Nested calls run `<parent>/<n>` with the parent attached.
- **Added:** eight opt-in hooks worlds, each declared in the manifest:
  `hooks-message` (`message_end` replace), `hooks-tool-call`
  (composable mutation/block), `hooks-tool-result` (composable
  results), `hooks-stream` (normalized observation, replayed in
  order), `hooks-settle` (`turn_end` / `agent_before_settle` with
  append-and-continue-once), `hooks-compaction` (veto + failure),
  `hooks-cache` (warming votes), `hooks-trust` (trust votes) (gh #45).
  The six-point `hooks` world is untouched.
- **Changed (compatible):** the `tool` world imports `lca:host/tools`.
  Imports are host-satisfied whether the guest uses them or not, so
  single-tool guests built before it keep linking (gh #77).

**Migration (cycle 4):** suites declare `tool-catalog` (and keep or
drop the single-tool world); orchestrators declare
`[capabilities.tools]` and call through the import; hooks declare the
worlds they handle. No migration for single-tool or six-hook guests:
they rebuild with no source change.

The train's first real interface break: the `ui` world redesign plus the
`ui-dialogs` host import. `lca:ext@0.6.0` / `lca:host@0.6.0`; the host
loads manifests declaring `0.5` or `0.6` (the window), but a 0.5-built
`ui` component that links the old widget shapes is refused legibly -
interface bytes changed, so same-line indistinguishability no longer
applies to the `ui` world.

- **Added:** `text-style` (fg/bg role-or-hex plus bold/dim/italic/
  underline) and the `styled-text`, `markdown`, `button`, `table`, and
  `scroll-container` widget cases (gh #172).
- **Changed (breaking):** `boxed` carries the border role and
  background tint (`tuple<option<string>, option<string>,
  option<string>, u32>`); the `interaction` input gains
  `click-widget`, `click`, and `scroll` (gh #172).
- **Added:** the `lca:host/ui-dialogs` import (`confirm`, `select`,
  `input`, `notify`), imported by the `tool`, `command`, `hooks`, and
  `ui` worlds (gh #124, gh #172).

**Migration:** rebuild against `@0.6.0`, declare `abi = "0.6"`, and
adjust `boxed` constructions plus the `interaction` handler's new cases
(the handler must be exhaustive over the `input` variant). A `ui`
component still built against 0.5 shapes does not link (its imports name
`@0.5.0`, which the 0.6 host no longer provides) and is refused with
the window message; other worlds' 0.5 components are unaffected where
their shapes did not change.

## 0.4.0 (release train, 2026-09-27)

The ABI label now tracks the product minor (ADR-0028's second annotation):
`lca 0.4.y` ships `abi 0.4`. No interface bytes changed between the 0.2 line
and this label; this train carries the in-place 0.2 changes below under the
0.4 label. The train's breaking item is `provider-models.list-models`'s new
`settings` parameter (ADR-0035).

**Migration (0.4.0):** rebuild against `@0.4.0`, declare `abi = "0.4"`, implement the
`settings` parameter, and push both registry tags. A component still
declaring `abi = "0.2"` is refused at the manifest check with a message
naming the accepted `0.3..=0.4` window - a legible refusal, not a link error.

## 0.2.0 (in-place development line, 2026-09-26 — superseded by the 0.4.0 relabel)

- **Breaking (in-place):** `provider-models.list-models` now takes
  `settings: list<extra-pair>`, the same opaque `setting: value` pairs
  `provider-completion` carries in its request `extras` (ADR-0035). A
  provider can now see its own persisted configuration when asked for
  models, instead of only the environment. A component built against the
  previous 0.2 shape fails to link and is refused at load.

Written for extension authors. Each entry names the version, the date, and
every change grouped as added, deprecated, removed, or fixed, with a
migration note for anything breaking (docs/abi-versioning.md).

## 0.2 development — closed (2026-09-25 to 2026-09-27)

The ADR-0028 development window's single **in-place** line. Breaking changes
land here without a minor bump (ADR-0028's annotation): the package stays
`@0.2.0`, every manifest stays `abi = "0.2"`, and this section is the running
record rather than one entry per change. The host loads 0.2, the previous line
(0.1), and the 1.0 freeze line, so nothing installed stops loading. The 1.0
entry below is the freeze's; 1.0 is a snapshot-and-relabel of the final 0.2
with no interface change between them.

### Changed (breaking)

- The `provider` world gains the `provider-login` export: `login-options`
  and `login-submit`, the host-rendered picker over the extension's own
  presets (ADR-0033). A provider component must export it; the two
  first-party providers and the conformance extension are rebuilt in the
  same change. **Migration:** implement `login-options`/`login-submit`,
  returning `login-result::ok` with no options if the provider has none.
- `types.message.content` is now `list<content-block>` instead of a joined
  `string`, and the new `content-block` variant carries `text(string)` or
  `image(tuple<string, list<u8>>)`. A provider can carry a typed image
  instead of text only (ADR-0029). **Migration:** read each block in order;
  the `text` blocks join to the old string, and `image` blocks are new. A
  rebuilt component declares `abi = "0.2"`.

### Added

- The `resources` host import (`lca:host/resources`): `list-resources` and
  `read`, the extension's own read-only `resources/` data bag (ADR-0030).
  Always available (like `log`), package-scoped, own tree only, per-call size
  capped; the same seam serves both delivery modes (ADR-0032). Imported by
  the `tool`, `provider`, `compaction`, `context-transform`, and `ui` worlds.
  Additive: a new host import interface, so an extension built against the
  previous WIT keeps loading; a rebuilt component declares `abi = "0.2"`.
- The `state` host import (`lca:host/state`): `read`, `write`, `delete`, and
  `list-keys`, the extension's own mutable, non-secret data bag (ADR-0030).
  Always available, identity-namespaced, size-capped, wiped on uninstall;
  not secret-grade (secrets go in `credentials`). Imported by the same five
  worlds. Additive, same as `resources`.
- The `content-block` variant in the `types` interface.

##1.0.0 — 2026-09-23

The freeze release: the ABI is now stable under semver, no breaking
change ships without2.0, and the pre-freeze punch list is closed out in
this one line - `provider` gains the `login`/`logout`/`usage` identity
exports (ADR-0012), the `compaction` and `context-transform` worlds
join the surface (ADR-0015), the `completion`, `net-local`, and `pty`
capabilities join the catalog (ADR-0015/0011/0016), `usage` carries
`cost`, every record carries `extras`, the widget vocabulary gains
`image` with its reserved `vendor` case, and the hook points are fixed
at `pre-turn`, `pre-tool-use`, `post-tool-use`, `post-turn-end`,
`attention-required`, `session-close`.

A host at1.0 loads extensions built for0.1 unchanged: that line is
the freeze grandfather (docs/abi-versioning.md).

##0.1.0 — the pre-freeze line (superseded by1.0.0)

The initial surface, ahead of the Phase 8 freeze at 1.0.

### Added

- Worlds `tool`, `command`, and `hooks` with the hook points fixed at
  `pre-turn`, `pre-tool-use`, `post-tool-use`, `post-turn-end`,
  `attention-required`, `session-close`.
- The `types` interface: `tool-call`, `tool-result`, `message`, `usage`
  (with `cache-read`, `cache-write`, `cache-write-hour` — the WIT spelling
  of the JSON `cache_write_1h`, since a WIT identifier may not start a
  segment with a digit — and `cost`), and
  `session-record`. Every record carries a reserved `extras` list of
  key-value pairs; WIT has no map type, so the SRDD's "map of string
  pairs" is realized as `list<extra-pair>`.
- Host import `lca:host/log`, always granted, message truncation at the
  configured limit (FR-EXT-10).
- Pending (added by their phases before the freeze, per the punch list in
  docs/abi-versioning.md): worlds `provider`, `ui`, `compaction`,
  `context-transform`; host imports `fs`, `net`, `net-local`, `oauth`,
  `credentials`, `process`, `pty`, `completion`, `ui`.

## 0.5.0 (release train, 2026-09-29)

The ABI label tracks the product minor (ADR-0028's second annotation):
`lca 0.5.y` ships `abi 0.5`. **No interface bytes changed between the 0.4
line and this label** — it is a relabel, carrying the same in-place 0.4
interface forward under the 0.5 label, exactly as the 0.4 train carried
0.2's changes forward.

**Migration:** rebuild against `@0.5.0` and declare `abi = "0.5"`. A
component still declaring `abi = "0.4"` keeps loading (the window accepts
the previous line); one declaring `abi = "0.3"` is refused at the manifest
check with a message naming the accepted `0.4..=0.5` window — a legible
refusal, not a link error.
