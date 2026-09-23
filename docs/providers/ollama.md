# Ollama provider

Version 0.1, 2026-09-20.

Source: `extensions/ollama/`. Delivery: WASM, installed separately, not bundled by default. Same access shape as LM Studio; documented separately because the wire format is not the same.

## What it authenticates against

Nothing, for the same reason as LM Studio: trust is "this is running on a machine I control," not a credential. Unlike LM Studio, Ollama's native API is its own shape, not the OpenAI chat completions format, so this provider's implementation does its own request and response mapping into the typed provider stream events rather than being a thin pass-through the way the OpenAI-compatible provider's local-server siblings could be if a service happens to mimic that format exactly.

## Manifest

```toml
name = "ollama"
version = "1.0.0"
abi = "1.0"
worlds = ["provider", "command"]
description = "Connects to a local Ollama server."

[capabilities.net-local]
addresses = ["127.0.0.1", "192.168.0.0/16", "*.local"]
```

Identical capability shape to LM Studio, for the identical reason: a local server needs `net-local` and nothing else.

## vendor-event usage

Model-loading and quantization metadata that Ollama's API returns and that has no equivalent in the typed provider events, carried through `vendor-event` rather than forcing a new case for something specific to one local runtime's API design.

## login, logout, usage

`login` and `logout` return "not supported." `usage` can report which model is currently loaded and, where Ollama's API exposes it, memory footprint, but has no token-spend concept.

## Why WASM by default

Same reasoning as LM Studio: optional, local-setup-specific, and not something the default binary should carry weight for.
