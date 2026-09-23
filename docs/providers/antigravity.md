# Antigravity provider

Version 0.1, 2026-09-20.

Source: `extensions/antigravity/`. Delivery: WASM, installed separately, not bundled by default. Reference implementation for the OAuth provider pattern; the design that ADR-0004's streaming shape and ADR-0009's update path were built to serve.

## What it authenticates against

Google's Antigravity API, using the loopback OAuth flow described in `docs/flows.md`. The extension builds the authorization URL and PKCE challenge, calls `oauth.begin` to get a redirect URL and open it, waits on `oauth.await` for the callback, and exchanges the code for tokens over `net`. It never binds a port itself; the host's loopback listener does that.

## Manifest

```toml
name = "antigravity"
version = "1.0.0"
abi = "1.0"
worlds = ["provider", "command"]
description = "Google Antigravity models, including image generation, via subscription login."

[capabilities.net]
hosts = ["generativelanguage.googleapis.com", "*.googleapis.com"]

[capabilities.oauth]
redirect_path = "/callback"

[capabilities.credentials]
namespace = "antigravity"
```

Every capability here is load-bearing: `net` names Google's API surface, one exact host plus one subdomain wildcard the API genuinely spans across model and media endpoints — a deliberate wildcard, called out here since the authoring guide asks for exact names wherever possible; `oauth` is required by the login flow; `credentials` stores exactly this provider's own tokens, unreadable by anything else, per the namespace isolation rule in the capability catalog.

## vendor-event usage

Carries image generation results and any Gemini-specific streaming detail the typed provider events don't have a dedicated case for, such as safety-filter metadata on a response. This is the exact situation `vendor-event` exists for: a real vendor concept that doesn't map onto text, reasoning, or a tool call, travelling through the reserved case rather than forcing an ABI break to add one.

## login, logout, usage

`login` runs the OAuth flow above and stores the resulting tokens. The OAuth client pair itself is user configuration, read from `ANTIGRAVITY_CLIENT_ID`/`ANTIGRAVITY_CLIENT_SECRET` (the same names pi uses for its own override) and stored into this extension's namespace on success so both delivery modes share it; LCA embeds no third party's client credentials, and a login with nothing configured reports which variables to set before any flow starts. `logout` clears them and, where the API supports it, revokes the token server-side rather than only discarding it locally. `usage` queries whatever usage or quota endpoint the API exposes and returns it in the standard usage-info shape ADR-0012 defines, which is what makes both `/antigravity.usage` and the generic `/usage`, when Antigravity is the active provider, work from the same implementation.

## Cache behavior

Google's context caching is an explicit, object-based mechanism rather than Anthropic's inline per-request marker: a cached content object is created and referenced by identifier on later calls, instead of a marker placed directly in the message list. This provider uses the host's cache-boundary hint to decide when the stable prefix has grown enough to be worth caching as its own object, and to know when to create a new one after a compaction changes it, rather than to place an inline marker the way an Anthropic-style provider would. It reports `cache_read` and `cache_write` from whatever the API's usage response calls the equivalent fields.

## Why WASM by default

Nothing about this provider needs to be trusted the way OpenAI-compatible's fresh-install role requires; it is exactly the kind of third-party-shaped, install-when-you-want-it extension the capability model exists for, and running it sandboxed costs nothing it needs. Its source is dual-mode like every extension under `extensions/`, so a build that wants it native-linked can still produce one; that is simply not what ships by default.
