# Antigravity provider

Version 0.1, 2026-09-20.

Source: `extensions/antigravity/`. Delivery: WASM, installed separately, not bundled by default. Reference implementation for the OAuth provider pattern; the design that ADR-0004's streaming shape and ADR-0009's update path were built to serve.

## What it authenticates against

Google's Antigravity API, using the loopback OAuth flow described in `docs/flows.md`. The extension builds the authorization URL and PKCE challenge, calls `oauth.begin` to get a redirect URL and open it, waits on `oauth.await` for the callback, and exchanges the code for tokens over `net`. It never binds a port itself; the host's loopback listener does that.

## Manifest

```toml
name = "antigravity"
version = "1.0.0"
abi = "0.6"
worlds = ["provider"]
description = "Google Antigravity's Gemini and Claude models, via subscription login."

[capabilities.net]
hosts = ["generativelanguage.googleapis.com", "*.googleapis.com"]

[capabilities.oauth]
redirect_path = "/callback"

[capabilities.credentials]
namespace = "antigravity"
```

Every capability here is load-bearing: `net` names Google's API surface, one exact host plus one subdomain wildcard the API genuinely spans across model and media endpoints — a deliberate wildcard, called out here since the authoring guide asks for exact names wherever possible; `oauth` is required by the login flow; `credentials` stores exactly this provider's own tokens, unreadable by anything else, per the namespace isolation rule in the capability catalog.

## vendor-event usage

Carries any Gemini-specific streaming detail the typed provider events don't have a dedicated case for, such as safety-filter metadata on a response. This is the exact situation `vendor-event` exists for: a real vendor concept that doesn't map onto text, reasoning, or a tool call, travelling through the reserved case rather than forcing an ABI break to add one.

Today the only vendor event this extension emits is `thinking-signature` (below), which rides that channel for replay. **Image generation is not implemented here** (gh #279): the provider streams text, reasoning, tool calls, usage and errors, and `lca_protocol::StreamEvent` has no variant an image result could arrive in. `ContentBlock::Image` exists, but it is the request side — an attached image becomes Gemini's `inlineData` part — which is image *input*, not generation. The vendor reference (`pi-antigravity`) does generate images, through a separate direct image endpoint plus a `generate_image` tool; LCA has neither, so the manifest does not advertise the capability.

## login, logout, usage

`usage` reports the subscription quota picture, not token counts (gh #241): the plan label from `loadCodeAssist` (`paidTier`, which holds the real subscription) plus normalized quota groups from `retrieveUserQuotaSummary`, carried in the `quota-plan` / `quota-groups` usage extras for the host's quota renderer (plan line, 20-wide bars, `% left · resets` countdowns, pi-antigravity's `formatUsageSummary` shape). A 403 on the quota endpoint records `quota-error: subscription-required` and still returns the plan — free tier cannot use that endpoint, and the renderer says so plainly.

`login` always runs the OAuth flow and stores the resulting tokens — even with tokens already stored, because a stored token may be expired or revoked and the invocation itself is the overwrite prompt (gh #179). Headless machines use `lca auth login --provider antigravity` (the paste shape in `docs/flows.md`); the token and auth endpoints are read from this extension's credential namespace, so a test IdP is one seeded file away. A 401 on any call purges the stored tokens, so the next call reports "no login" and re-authenticates instead of retrying the dead token. When a next-gen preview id 404s, the call falls back to the mapped backend model (`gemini-3.8-flash*` → `gemini-3.7-flash*`, `gemini-3.7-flash*` → `gemini-3.6-flash*`, pi's `getFallbackRuntimeModel` as data). The OAuth client pair itself is user configuration, read from `ANTIGRAVITY_CLIENT_ID`/`ANTIGRAVITY_CLIENT_SECRET` (the same names pi uses for its own override) and stored into this extension's namespace on success so both delivery modes share it; LCA embeds no third party's client credentials, and a login with nothing configured reports which variables to set before any flow starts. `logout` clears them and, where the API supports it, revokes the token server-side rather than only discarding it locally.

**No `[capabilities.env]` section, deliberately** (gh #484). The `ANTIGRAVITY_CLIENT_ID`/`ANTIGRAVITY_CLIENT_SECRET` reads sit under `#[cfg(not(target_arch = "wasm32"))]`, so the sandboxed build never executes them; it reads the *stored* pair from this extension's credential namespace, which `login` writes in either delivery mode. Declaring the grant would forward a client secret the guest has no code path to read. Note also what these are: **OAuth application credentials, not a user's key** — pi's own override names, used so a developer can point this extension at their own Google app without rebuilding. The compiled-in defaults are a public desktop client's constants (see `client_pair`), which is why the sandboxed build works with nothing configured. `tests/regressions/484-*.rs` keeps the gate honest. `usage` queries whatever usage or quota endpoint the API exposes and returns it in the standard usage-info shape ADR-0012 defines, which is what makes both `/antigravity.usage` and the generic `/usage`, when Antigravity is the active provider, work from the same implementation.

## Reasoning signatures across the ABI

Gemini's `generateContent` rejects a replayed `functionCall` part whose `thoughtSignature` is missing (HTTP 400), so every tool turn's second provider round depends on that signature surviving the trip out and back. The host persists it off the stream as a `Reasoning` block (gh #41), but the frozen 0.6 `content-block` variant has no `reasoning` case — so it crosses the ABI in the message's reserved extras map under `thinking-signature`, and the WASM guest rebuilds the block from it before building the request. On replay the signature rides as a **sibling** of `functionCall` on the part, which is the exact shape the stream delivers it in: nested inside `functionCall`, this gateway rejects it as an unknown field in both camelCase and snake_case. An unsigned thinking run sends no `thoughtSignature` key at all rather than an empty one.

Its scope is the tool-turn case, which is what the bridge was built for: one signature per assistant message rides on replay inside the `functionCall` parts. A signed turn with **no** tool call replays without the signature (the echo is gated on a call existing) — pi-antigravity additionally echoes a per-part `textSignature` on text parts, and LCA does not model that separate slot. That is a known fidelity boundary, not a regression, and it is recorded in gh #289's report for an owner decision rather than changed inside the 0.6 record.

**Live-verified 2026-10-17** (gh #289): `gemini-3.8-flash` tool turns complete and follow-ups answer over this bridge. The bridge is the signature transport for the whole 0.6 line; the typed `reasoning` WIT case is 0.7 work tracked in #352, and `docs/abi-versioning.md` records the 0.6-compat window around it.

## Cache behavior

Google's context caching is an explicit, object-based mechanism rather than Anthropic's inline per-request marker: a cached content object is created and referenced by identifier on later calls, instead of a marker placed directly in the message list. This provider uses the host's cache-boundary hint to decide when the stable prefix has grown enough to be worth caching as its own object, and to know when to create a new one after a compaction changes it, rather than to place an inline marker the way an Anthropic-style provider would. It reports `cache_read` and `cache_write` from whatever the API's usage response calls the equivalent fields.

## Why WASM by default

Nothing about this provider needs to be trusted the way OpenAI-compatible's fresh-install role requires; it is exactly the kind of third-party-shaped, install-when-you-want-it extension the capability model exists for, and running it sandboxed costs nothing it needs. Its source is dual-mode like every extension under `extensions/`, so a build that wants it native-linked can still produce one; that is simply not what ships by default.
