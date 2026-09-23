# ADR-0023: The OpenAI-compatible provider sends `x-opencode-session`

Status: accepted.

Date: 2026-09-23.

## Context

OpenCode Go is an OpenAI-shaped endpoint the built-in provider points at directly, with no dedicated extension (`docs/providers/README.md`). The worker brief warned that its dialect has quirks worth verifying on the first smoke call, one of them being a per-conversation `x-opencode-session` header that their reference implementation treats as required. The first real smoke call against `https://opencode.ai/zen/go/v1` confirmed it: the endpoint answers HTTP 400, "Request is missing x-opencode-session and cannot be routed efficiently."

## Decision

The core puts the session identifier into `CompletionRequest.extras["session-id"]` on every request, and the OpenAI-compatible provider forwards it as `x-opencode-session`. One header, sent unconditionally: unknown headers are ignored by every other OpenAI-shaped server, which is most of this provider's users, and branching on the host would be exactly the vendor special-casing ADR-0013 keeps out of the core - the quirk lives at the edge, inside the provider whose whole job is speaking this dialect.

## Alternatives considered

Send the header only when the base URL is OpenCode's. Rejected: it reintroduces endpoint identity into a provider defined as "any OpenAI-compatible endpoint", and no existing OpenAI-compatible server rejects an extra header.

A dedicated OpenCode extension with the header in its manifest or WIT surface. Rejected by the brief itself ("the fix is small, NOT a new extension"), and `docs/providers/README.md` already states OpenCode Go needs no dedicated extension.

Carry the session id in the WIT surface instead of `extras`. Unnecessary for now - `extras` is precisely the reserved map for non-structural additions like this (docs/abi-versioning.md), and provider-world changes are breaking before the freeze and doubly so after.

## Consequences

A real OpenCode Go turn completes (verified by the env-gated smoke test); other endpoints see one extra harmless header. The session id crosses the provider boundary as a string in `extras`, which is also how a future provider that wants conversation identity gets one without an ABI change.

## Revisit conditions

A provider that treats unknown headers as an error, or a need to namespace the session id per provider.
