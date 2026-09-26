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
extension removes its presets with it. The `provider-identity` `login` stays
for providers whose flow is self-contained (antigravity's OAuth). Conformance
gains options round-trip and submit cases in both delivery modes; the host
gains one new dispatch method pair. Wire identity (ADR-0031's V1/V2) is
unchanged by this record.
