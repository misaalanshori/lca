# Provider extensions

Version 0.1, 2026-09-20.

Every model provider is an extension implementing the `provider` world, none of them special-cased in the core, per the crate decomposition in the main document. This directory documents the first-party ones: what each authenticates against, exactly which capabilities it declares and why, anything vendor-specific it carries through `vendor-event`, and its delivery mode.

A full ADR is not written for each provider, because there is usually no real alternative being weighed; a provider profile is closer to a specification than a decision. Where a provider's design did settle a real question, its document links to the ADR that owns it rather than repeating the reasoning.

## Index

| Provider | Auth | Delivery mode by default | Notes |
|---|---|---|---|
| [OpenAI-compatible](openai-compatible.md) | API key | Native-linked, enabled | Ships in the binary. Any base URL speaking the OpenAI chat completions shape. |
| [Antigravity](antigravity.md) | OAuth | WASM, install separately | Reference implementation for the OAuth pattern; see ADR-0004, ADR-0009. |
| [Codex](codex.md) | OAuth | WASM, install separately | Same shape as Antigravity, second proof the pattern generalizes. |
| [LM Studio](lmstudio.md) | None | WASM, install separately | Local server, `net-local`; see ADR-0011. |
| [Ollama](ollama.md) | None | WASM, install separately | Local server, `net-local`; see ADR-0011. |

OpenCode Go is deliberately absent from this list. It exposes an OpenAI-compatible endpoint with its own base URL and key, so it needs no dedicated extension; a user points the OpenAI-compatible provider at it directly.

## Template

A provider document covers, in order: what it authenticates against and how; its full manifest, including every capability with a one-line justification for each; anything it carries through `vendor-event` and why the typed cases didn't cover it; its `login`, `logout`, and `usage` behavior per ADR-0012; and its delivery mode, native-linked or WASM, with the reason if it differs from what the capability set alone would suggest.

Every capability line in a provider's manifest should be traceable to something the provider concretely does. A provider document that cannot say what a declared capability is for has declared too much.
