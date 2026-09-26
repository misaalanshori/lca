# ADR-0035: `list-models` takes the settings `complete` does

Date: 2026-09-26. Status: accepted. Window: in-place on the 0.2 line
(ADR-0028).

## Context

`provider-completion.complete` receives the host-persisted `setting: value`
pairs on every call, in its request `extras`. `provider-models.list-models`
took nothing. The result was an asymmetry with a real cost: a provider
could discover an endpoint's model list during `login-submit`, hand it back
as an opaque `models` setting for the host to persist - and then not see it
when the host asked for models. The WASM path could only read the
environment; the native path papered over the gap by reading its own
credential store, which is a **second** store, so the two delivery modes
did not even agree on where the truth lives.

The kink was recorded as "WASM `list-models` cannot see the stored model
list (the WIT function takes no capability handle)". That diagnosis named
the symptom, not the cause. The cause is the missing argument.

## Decision

`list-models` takes the same settings flow `complete` does:

```wit
list-models: func(settings: list<extra-pair>) -> list<model-info>;
```

The host passes what it would put in `complete`'s `extras`. The extension
reads its configuration from those pairs and nothing else; the discovered
model list lives in host-persisted settings with **no second store**.

Rust follows the WIT: `ExtensionDispatch::provider_models` gains the same
parameter, and the native implementations prefer the passed settings over
anything they could read themselves.

This is a breaking change on the 0.2 in-place line, which is exactly what
the window exists for: mold the shape right before the freeze rather than
let a workaround bake a wrong shape into 1.0.

## Consequences

- Both delivery modes answer from one source, so the conformance diff can
  actually compare them.
- A component built against the previous 0.2 shape fails to link and is
  refused at load - the ordinary window behavior, not an emergency.
- `ExtensionProvider` carries the settings the host persisted and passes
  them on the call; the host is still the only writer of that file.

## Alternatives considered

- **The `state` bag fallback** (extension writes the list to its own state,
  `list-models` reads it there). No ABI cost, sanctioned as a fallback, and
  it does fix the behavior. Rejected as the primary: it introduces the
  second store the symmetry exists to remove, and it leaves the wrong
  signature to freeze.
- **A capability handle for `list-models`** (so it can read its own
  credentials). Rejected: it makes the answer depend on what the extension
  happens to have stored, which is the current bug.
- **Do nothing and document the env-only shape.** Rejected: the window is
  open precisely so this is cheap now and expensive later.
