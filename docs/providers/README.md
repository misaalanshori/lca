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

## Presets

`openai-compatible` ships its picker entries as its own data,
`resources/provider-presets.toml` (ADR-0031), not as host code. Disabling
the extension takes the list with it; a user's own entries live at
`<config>/provider-presets.toml` and are merged into the same picker.

19 presets ship today:

| id | Name | Base URL | Auth |
|---|---|---|---|
| `openai` | OpenAI | `https://api.openai.com/v1` | `bearer` |
| `openrouter` | OpenRouter | `https://openrouter.ai/api/v1` | `bearer` |
| `deepseek` | DeepSeek | `https://api.deepseek.com/v1` | `bearer` |
| `groq` | Groq | `https://api.groq.com/openai/v1` | `bearer` |
| `cerebras` | Cerebras | `https://api.cerebras.ai/v1` | `bearer` |
| `fireworks` | Fireworks | `https://api.fireworks.ai/inference/v1` | `bearer` |
| `together` | Together | `https://api.together.xyz/v1` | `bearer` |
| `xai` | xAI | `https://api.x.ai/v1` | `bearer` |
| `mistral` | Mistral | `https://api.mistral.ai/v1` | `bearer` |
| `moonshot` | Moonshot | `https://api.moonshot.ai/v1` | `bearer` |
| `minimax` | MiniMax | `https://api.minimax.io/v1` | `bearer` |
| `nvidia` | NVIDIA | `https://integrate.api.nvidia.com/v1` | `bearer` |
| `baseten` | Baseten | `https://inference.baseten.co/v1` | `bearer` |
| `huggingface` | Hugging Face | `https://router.huggingface.co/v1` | `bearer` |
| `github-models` | GitHub Models | `https://models.inference.ai.azure.com` | `bearer` |
| `opencode-go` | OpenCode Go | `https://opencode.ai/zen/go/v1` | `bearer` |
| `perplexity` | Perplexity | `https://api.perplexity.ai` | `bearer` |
| `ollama` | Ollama (local) | `http://localhost:11434/v1` | `none` |
| `lmstudio` | LM Studio (local) | `http://localhost:1234/v1` | `none` |

### Declaring a preset

A preset is one `[[preset]]` block. The host never parses these; the
extension answers `login-options` from them and nothing else reads the
shape.

```toml
[[preset]]
id = "my-proxy"              # stable; `/login my-proxy` selects it directly
name = "My Proxy"            # what the picker shows
base_url = "https://llm.example.com/v1"
auth = "bearer"              # "bearer" asks for a key; "none" has no key step
models = ["a", "b"]          # curated fallback; the live list wins if reachable
```

`auth = "none"` is the local-endpoint shape (Ollama, LM Studio): choosing
one signs straight in with no key prompt. A base URL whose host is outside
the manifest's `net` vocabulary gets the ad hoc grant prompt naming that
host (FR-PERM-16).
