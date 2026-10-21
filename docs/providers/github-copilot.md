# GitHub Copilot provider

Source: `extensions/github-copilot/`. Delivery: WASM, installed separately, not bundled by default. The device-flow proof: no loopback listener, no pasted callback — the extension polls GitHub to completion, then mints short-lived Copilot tokens from the device grant.

## What it authenticates against

A Copilot Individual/Business/Enterprise subscription through GitHub's device flow (pi's `packages/ai/src/auth/oauth/github-copilot.ts`): the extension fetches a device code, the host shows the code beside `github.com/login/device` (the `#code=` fragment convention — the only freeze-safe display channel, no new host import), the extension polls `github.com/login/oauth/access_token` past `authorization_pending`, then exchanges the GitHub token at `api.{domain}/copilot_internal/v2/token`. An enterprise domain rides the one login field (empty means github.com); custom domains stay out of the manifest and ride the ad-hoc grant. The GitHub token persists as the mint source; the Copilot token carries a five-minute safety margin and re-mints on demand.

## Manifest

```toml
name = "github-copilot"
version = "0.1.0"
abi = "0.6"
worlds = ["provider"]
description = "GitHub Copilot subscription provider."

[capabilities.net]
hosts = [
  "github.com",
  "api.github.com",
  "api.githubcopilot.com",
  "api.individual.githubcopilot.com",
  "api.business.githubcopilot.com",
  "api.enterprise.githubcopilot.com",
]

[capabilities.credentials]
namespace = "github-copilot"
```

Two notes against the September spec: `worlds` is `provider` only (FR-PROV-10, the codex note), and there is deliberately no `[capabilities.oauth]` section — a device flow binds no loopback listener.

## Requests

Chat bodies over `POST {base}/chat/completions` where the base comes from the token's `proxy-ep` (`proxy.` becomes `api.`, pi's rule), then the enterprise shape, then the individual default: the bearer, pi's Copilot header block (`User-Agent`, `Editor-Version`, `Editor-Plugin-Version`, `Copilot-Integration-Id`), and `X-GitHub-Api-Version`. Model-policy auto-enabling (pi's best-effort POSTs) stays out of the login path — enabling happens on github.com.

## Models

`GET {base}/models` is the account's own catalog: picker-enabled and policy-enabled rows list, disabled and tool-less ones do not (gh #184). Two sources size each row, in precedence order — the same discipline `openai-compatible`'s four-source ladder applies, narrowed to what this gateway publishes:

1. **The curated table** (`CURATED_MODELS`) wins for an id it knows. Its figures are vendor-documented; the endpoint's are the account's view and can be a narrower tier, so `gpt-4o` reports the table's 128000 even if the account is offered 64000. A wrong denominator silently mis-triggers compaction (FR-SESS-4), which is worse than a missing one.
2. **The endpoint's declared window** — `capabilities.limits.max_context_window_tokens`, falling back to `max_prompt_tokens` when only that is present — for an id the table does not know, so a newly-offered model is sized from the only figure published about it rather than reported unknown.
3. **`0`** when neither source answers, honestly. The footer keeps `ctx ?` and compaction stays off; this provider never invents a number, because a fabricated window is the failure mode the whole ladder exists to prevent.

The same mapping keeps the rest of what the endpoint publishes: `max_output_tokens` rides `ModelInfo.max_tokens`, the display `name` replaces the echoed id (blank falls back to the id), and the capability hints ride the documented `extras` vocabulary — `capabilities.supports.vision` becomes `image.vision` (gh #39) and `capabilities.supports.reasoning_effort` joins into `thinking_levels` (gh #242), so the picker offers only the levels the account's model accepts. A hint the endpoint omits stays **absent**, which means unknown; it is never synthesized as `false`. Copilot ships as a component, so the guest forwards these extras across the WIT boundary (gh #283) — the record always carried them.

The offline floor is the five curated rows, all fully sized.

## login, logout, usage

`login` offers the one device choice (enterprise domain optional). `logout` clears the namespace (device grants die on the site). `usage` is the credential-validity probe — a live token is `Ok` with empty counts.

## Limits

Device turns never run in CI — the suite drives the mock gateway (pending-then-token polls, the mint, the catalog, the chat fixture), and the live smoke skips without an operator-provided token (NFR-23).
