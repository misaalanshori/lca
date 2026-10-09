# Provider extensions

Version 0.1, 2026-09-20.

Every model provider is an extension implementing the `provider` world, none of them special-cased in the core, per the crate decomposition in the main document. This directory documents the first-party ones: what each authenticates against, exactly which capabilities it declares and why, anything vendor-specific it carries through `vendor-event`, and its delivery mode.

A full ADR is not written for each provider, because there is usually no real alternative being weighed; a provider profile is closer to a specification than a decision. Where a provider's design did settle a real question, its document links to the ADR that owns it rather than repeating the reasoning.

## Index

| Provider | Auth | Delivery mode by default | Notes |
|---|---|---|---|
| [OpenAI-compatible](openai-compatible.md) | API key | Native-linked, enabled | Ships in the binary. Any base URL speaking the OpenAI chat completions shape. |
| [Antigravity](antigravity.md) | OAuth | WASM, install separately | Reference implementation for the OAuth pattern; see ADR-0004, ADR-0009. |
| [Codex](codex.md) | OAuth | WASM, install separately | ChatGPT subscription via the Codex responses gateway; shares the subscription kit with Grok. |
| [Grok](grok.md) | OAuth | WASM, install separately | SuperGrok / X subscription via the Grok responses proxy; shares the subscription kit with Codex. |
| [Anthropic](anthropic.md) | API key + OAuth | WASM, install separately | Claude via Messages: cache breakpoints, thinking signatures, Pro/Max subscription (browser + copy-code) beside a plain key. |
| [OpenRouter](openrouter.md) | OAuth (provisions key) | WASM, install separately | One-click browser OAuth on the shared OpenAI wire kit; `openai-compatible` stays key-only. |
| [GitHub Copilot](github-copilot.md) | Device code | WASM, install separately | Copilot subscription via the device flow; token `proxy-ep` routes inference. |
| [LM Studio](lmstudio.md) | None | WASM (specified, not in the tree) | Local server, `net-local`; see ADR-0011. |
| [Ollama](ollama.md) | None | WASM (specified, not in the tree) | Local server, `net-local`; see ADR-0011. |

The last two rows are specifications, not shippable artifacts: the profiles are complete, nothing under `extensions/` builds them, and the release publishes only `openai-compatible`, `antigravity`, `codex`, `grok`, `skills`, and `compaction-default` to the registry (`.github/workflows/publish.yml`).

Policy (gh #21): LM Studio and Ollama stay spec-only deliberately — the supported route for local models is the `ollama` and `lmstudio` presets (`auth = "none"`) through the `openai-compatible` provider, whose wire shape both servers' `/v1` endpoints speak. Codex graduated from the spec-only list in the provider-breadth cycle (gh #63): a ChatGPT subscription login needs its OAuth extension, and now it has one.

OpenCode Go is deliberately absent from this list. It exposes an OpenAI-compatible endpoint with its own base URL and key, so it needs no dedicated extension; a user points the OpenAI-compatible provider at it directly.

## Pi parity table (gh #63)

Every pi provider (`packages/ai/src/providers/`) against LCA status.
`preset` rows ride the `openai-compatible` picker above; `absent`
means no demand has been voted yet, not a verdict — the next demand
votes get counted against this table.

| pi provider | LCA status | Notes |
|---|---|---|
| pi `openai` | preset | `openai` row, `OPENAI_API_KEY`. |
| pi `openrouter` | preset + extension | `openrouter` row for the key; `openrouter` for the one-click OAuth that provisions one. |
| pi `deepseek` | preset | `deepseek` row, `DEEPSEEK_API_KEY`. |
| pi `groq` | preset | `groq` row, `GROQ_API_KEY`. |
| pi `cerebras` | preset | `cerebras` row. |
| pi `fireworks` | preset | `fireworks` row. |
| pi `together` | preset | `together` row. |
| pi `mistral` | preset | `mistral` row. |
| pi `moonshotai` | preset | `moonshot` row. |
| pi `minimax` | preset | `minimax` row. |
| pi `nvidia` | preset | `nvidia` row. |
| pi `baseten` | preset | `baseten` row. |
| pi `huggingface` | preset | `huggingface` row. |
| pi `opencode-go` | preset | `opencode-go` row (the Go endpoint only). |
| pi `vercel-ai-gateway` | preset | `vercel` row, `AI_GATEWAY_API_KEY` (gh #182: preset, not an extension). |
| pi `openai-codex` | extension | `codex`: ChatGPT subscription OAuth + responses gateway. |
| `xai` (API key) | preset + extension | `xai` row for the key; `grok` for the SuperGrok subscription OAuth + responses proxy. |
| pi `github-copilot` | extension | `github-copilot`: device flow, token mint, Copilot headers; the `github-models` preset still covers only the Models endpoint. |
| pi `kimi-coding` | absent | No demand yet. |
| pi `meta` | absent | No demand yet. |
| pi `google` (Gemini API) | absent | Gemini's own API shape; no preset yet. |
| pi `anthropic` | extension | `anthropic`: API key + Pro/Max OAuth (browser + copy-code), Messages with cache breakpoints and signatures. |
| pi `azure` | absent | Entra + endpoint shape; no demand yet. |
| pi `amazon-bedrock` | absent | Ambient AWS credentials; no demand yet. |
| pi `google-vertex` | absent | Ambient GCP credentials; no demand yet. |
| pi `minimax-cn` + `moonshotai-cn` | absent | CN endpoints; the global rows do not cover them. |
| pi `ant-ling` | absent | No demand yet. |
| pi `cloudflare-ai-gateway` | absent | A gateway like Vercel; no preset yet. |
| pi cloudflare set (`workers-ai`, `auth`, `stream`) | absent | Infrastructure pieces, not model providers. |
| pi token-plan/regional set (`qwen`, `xiaomi`, `zai`, `typesafe`, `radius`) | absent | No demand yet. |
| pi `opencode` | absent | The harness protocol, not an endpoint (covered via `opencode-go`). |
| — | LCA-only: `antigravity` | Google subscription OAuth; pi has no counterpart (it lives in pi-antigravity). |

## Template

A provider document covers, in order: what it authenticates against and how; its full manifest, including every capability with a one-line justification for each; anything it carries through `vendor-event` and why the typed cases didn't cover it; its `login`, `logout`, and `usage` behavior per ADR-0012; and its delivery mode, native-linked or WASM, with the reason if it differs from what the capability set alone would suggest.

Every capability line in a provider's manifest should be traceable to something the provider concretely does. A provider document that cannot say what a declared capability is for has declared too much.

## Presets

`openai-compatible` ships its picker entries as its own data,
`resources/provider-presets.toml` (ADR-0031), not as host code. Disabling
the extension takes the list with it; a user's own entries live at
`<config>/provider-presets.toml` and are merged into the same picker.

21 presets ship today:

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
| `vercel` | Vercel AI Gateway | `https://ai-gateway.vercel.sh/v1` | `bearer` |
| `ollama` | Ollama (local) | `http://localhost:11434/v1` | `none` |
| `lmstudio` | LM Studio (local) | `http://localhost:1234/v1` | `none` |
| `custom` | Custom endpoint… | *(you supply it)* | `bearer` |

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
