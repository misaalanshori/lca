# ADR-0033: The provider world's login surface

Status: accepted.

Date: 2026-09-26.

## Context

ADR-0031 settled *where* preset knowledge lives (the extension's own
`resources/` bag) and *what the host is* (UI + courier + consent). What
remained was the interface between them: how the host asks an extension for
its login options and hands back the user's answers, without the host ever
parsing provider-shaped data.

The `provider` world already exports `login`/`logout`/`usage`
(ADR-0012's optional-export rule): a provider runs its own flow. That works
for a provider that knows its endpoint, but pi's `/login` is a *picker* over
known endpoints, and the picker is the host's chrome. A bare `login()` gives
the host nothing to render.

## Decision

The `provider` world gains `provider-login`:

```wit
login-options: func() -> list<login-option>;
login-submit: func(answer: login-answer) -> login-result;
```

`login-options` returns display-ready choices (`id`, `name`, `kind`,
`host`, `fields`, `extras`); `login-submit` consumes the chosen id and the
field values, stores the secret in the extension's own `credentials`
namespace, and returns either `ok` or opaque `setting: value` pairs for the
host to persist.

The extension loads its options from `resources/provider-presets.toml`
through `lca:host/resources` (ADR-0030); the host renders them and ferries
the answers. The host always appends its own universal "Custom endpoint…"
entry (base URL + key + model), which needs no preset. The ad hoc `net`
grant (FR-PERM-16) fires when a chosen `host` is outside the manifest's
fixed vocabulary, showing the exact host.

Adding an export to a world is breaking (a component must export it). That
is allowed in the 0.2 window (ADR-0028) with the usual discipline: this ADR,
a `wit/CHANGELOG.md` line, and the conformance extension updated in the same
change. The first-party providers rebuild; there is no third-party ecosystem
to whiplash.

## Alternatives considered

- **A new `login` world the extension opts into.** The host would have to
  discover which providers export it and reconcile two provider surfaces.
  The provider world is where provider identity already lives. Rejected.
- **Host-side presets.** Re-litigates ADR-0031. Rejected.
- **Extend `login()` with an argument.** `login` is the "run my own flow"
  operation (OAuth); overloading it with a picker protocol muddies both.
  Rejected.

## Consequences

`/login` becomes a picker fed by installed provider extensions; disabling an
extension removes its presets with it. **Option resolution is provider-scoped
(annotated 2026-10-18, gh #265):** several providers declare the same option
ids (`api-key`, `oauth`), so a choice is matched *inside the provider that
offers it* and the matched option's owner is the credential namespace. An
id-only search is a cross-vendor credential leak — the key lands in whichever
provider declared that id first, and that provider then gets sent it. The
id-only fallback survives only for the bare `/login` pick, whose rows carry
their own provider anyway; a miss under a named provider is a named refusal,
never a silent reroute to someone else's option. The `provider-identity` `login` stays
for providers whose flow is self-contained (antigravity's OAuth). Conformance
gains options round-trip and submit cases in both delivery modes; the host
gains one new dispatch method pair. Wire identity (ADR-0031's V1/V2) is
unchanged by this record.

## Annotation — 2026-10-21 (gh #297): a provider whose sign-in is its own flow is declared, not discovered

This record defines the picker as fed by `login-options`. That left an
unstated case, and the interface resolved it silently and wrongly: a
provider that signs in through `provider-identity` `login` (an OAuth or
device-code dance it runs itself) declares no presets, so it contributed no
picker row at all. `/login` therefore listed only the preset providers;
`antigravity`, `codex`, `grok`, `kimi-coding`, `meta` (and `openrouter`,
same shape) were reachable only by knowing the undocumented
`/login <name>` spelling.

Two additions, both within this record's split:

1. **`[login] identity = true` in the manifest** declares that a provider
   signs in through its own flow. The host lists such a provider with one
   synthesized row (`Sign in with <name>`, kind `oauth`, no fields) and
   routes that pick to `provider-identity` `login`. The declaration lives
   in the manifest because the manifest is already the consent surface, and
   because the alternative — *asking* a provider whether it can sign in, by
   calling `identity_login()` and reading `NotSupported` — **runs the
   flow**: it opens a browser or blocks on a device poll for every
   installed subscription provider the moment `/login` opens. That was
   implemented, measured, and reverted; it hung the picker.
2. **The pick path is now two-valued.** A choice carrying the synthesized
   row's id goes to the provider's own `login`; everything else goes to
   `login-submit` as before. This half is load-bearing rather than
   cosmetic: none of these providers implements `login-submit`, whose trait
   default returns `Ok(vec![])` having stored nothing, so a synthesized row
   routed down the submit path would report a completed sign-in and leave
   the user with no credential — worse than the invisible row it replaced.

Consequences: a provider that declares an identity flow and forgets nothing
else is visible with no host change, so a future provider cannot be hidden
the same way; a provider that declares no flow still contributes no row
(`llama` is keyless until configured); and a provider with declared presets
is untouched — the rule only fills in providers that declared none. The
manifest schema's `login` table is additive by design ("unknown keys are
ignored by older hosts"), so the key needs no ABI line.

