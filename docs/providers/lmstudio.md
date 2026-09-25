# LM Studio provider

Version 0.1, 2026-09-20.

Source: `extensions/lmstudio/`. Delivery: WASM, installed separately, not bundled by default. The first-party proof that `net-local`, added by ADR-0011, actually serves a real provider rather than a hypothetical one.

## What it authenticates against

Nothing. LM Studio's local server needs no credential; the trust boundary is that it is running on a machine the user controls, not that it checked an API key. The extension speaks LM Studio's OpenAI-compatible local endpoint, which is the same wire shape the OpenAI-compatible provider speaks, just reached over plain HTTP to a local address instead of HTTPS to a public one.

## Manifest

```toml
name = "lmstudio"
version = "1.0.0"
abi = "0.2"
worlds = ["provider", "command"]
description = "Connects to a local LM Studio server."

[capabilities.net-local]
addresses = ["127.0.0.1", "192.168.0.0/16", "*.local"]
```

No `credentials` capability, since there is nothing to store. No `oauth`, for the same reason. The `net-local` grant is the entire access surface this provider needs, which is a useful contrast with Antigravity and Codex: a local provider is, in a real sense, a narrower-permission extension than an OAuth one, even though it is reaching a wider address range than a single pinned hostname.

## vendor-event usage

None expected in practice, since the endpoint deliberately mimics the OpenAI shape.

## login, logout, usage

`login` and `logout` return "not supported"; there is no account to sign into. `usage` can report locally-known information, such as which model is currently loaded in LM Studio, if the local API exposes it, but has no meaningful token-spend concept to report the way a hosted provider's usage does.

## Why WASM by default

Local providers are exactly the shape of extension the sandbox was designed to make safe to install casually: they are optional, user-specific, tied to a particular local setup, and shipping them all natively would bloat the default binary with providers most installs will never use.

## Connecting to a machine that isn't localhost

The `192.168.0.0/16` range in the example manifest is what makes the laptop-reaching-desktop case work: a user running LM Studio on a separate machine on their own network can point this extension at that machine's address without needing a broader grant than the local network it is actually on. The `*.local` entry covers the same case by mDNS name rather than a memorized IP, when the desktop advertises one.
