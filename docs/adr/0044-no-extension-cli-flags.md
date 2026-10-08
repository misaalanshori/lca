# ADR-0044: No dynamic CLI flags or shortcuts in the extension ABI

Status: accepted.

Date: 2026-10-09.

## Context

Pi extensions shape the command line and the keyboard at runtime:
`registerFlag` adds a long-form option (`pi --plan` from a plan-mode
extension, boolean or string with a default), and `registerShortcut`
binds a key to a handler with a description (gh #79, PG-027/PG-054).
LCA's CLI is static clap parsed before anything loads
(`crates/lca-cli/src/cli_args.rs`), and its keymap is a static table
with user overrides (`lca-config/src/keys.rs`). The 0.6 freeze holds:
no `wit/` or `schemas/` change on this line. The question is whether
an extension should be able to add flags and shortcuts anyway — and
if not, where those needs go.

## Decision

**Extensions get neither dynamic flags nor dynamic shortcuts.**
Per-extension knobs are typed config keys; per-extension actions are
slash commands (the `command` world, shipped) and, when it lands,
`lca <ext>` delegation (gh #171, deferred to 0.7). Keybindings stay
a user-owned static map: extensions cannot add keys.

## Alternatives considered

Manifest-declared flags parsed before clap. Rejected: every
installed extension would change `--help`, shell completions, and
error text, so the CLI surface stops being deterministic; and the
manifest schema is frozen, so the declaration has nowhere to live
until 0.7 — at which point the objections below still hold.

Full dynamic registration (pi's shape). Rejected twice over. Flags:
registration must run before parsing, which inverts startup order
(load extensions, then parse, then run) and buys a whole failure
class (an extension that fails to load breaks flag parsing for
unrelated commands). Shortcuts: two extensions claiming one key
needs arbitration UI that does not exist; silent last-wins or
first-wins both lie to the user.

Deferring the call to 0.7. Rejected: the objections are structural,
not scheduling. A new ABI line does not make `--help`
deterministic.

## Consequences

PG-027/PG-054 close as a settled divergence, recorded in
`docs/pi-parity.md`: a parity case demanding an extension flag is a
wrong case. `docs/extension-authoring.md` points flag-shaped needs
at config keys (typed, documented, `/settings`-visible, file > env >
flag precedence through the settings ladder). Nothing is
implemented beyond those two doc rows: no WIT, no schema, no clap
change. An extension that needs flag-shaped input today asks for a
config key through the normal settings process.

## Revisit conditions

Two pieces of evidence together overturn this: `lca <ext>`
delegation ships (gh #171), AND a concrete extension demonstrates a
need that config keys plus delegation cannot express (a flag that
changes startup behavior before config loads, with the extension
named). Either alone does not. A revisit is a new ADR; this one is
never edited.
