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

Chat bodies over `POST {base}/chat/completions` where the base comes from the token's `proxy-ep` (`proxy.` becomes `api.`, pi's rule), then the enterprise shape, then the individual default: the bearer, pi's Copilot header block (`User-Agent`, `Editor-Version`, `Editor-Plugin-Version`, `Copilot-Integration-Id`), and `X-GitHub-Api-Version`. Model-policy auto-enabling (pi's best-effort POSTs) stays out of the login path — enabling happens on github.com; the live catalog lists picker-enabled and policy-enabled rows and skips disabled and tool-less ones, with the issue's five rows as the offline floor.

## login, logout, usage

`login` offers the one device choice (enterprise domain optional). `logout` clears the namespace (device grants die on the site). `usage` is the credential-validity probe — a live token is `Ok` with empty counts.

## Limits

Device turns never run in CI — the suite drives the mock gateway (pending-then-token polls, the mint, the catalog, the chat fixture), and the live smoke skips without an operator-provided token (NFR-23).
