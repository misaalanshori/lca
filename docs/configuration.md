# Configuration reference

Version 0.1, 2026-09-20.

This is the key reference for the merged configuration the requirements document summarizes. It names every key, its type, its default, and which source can set it. `lca config` prints the resolved value of each key and the source that set it, per FR-CFG-2.

## Sources and precedence

Highest first: command line flags, environment variables, the project file at `.lca/config.toml`, the user file in the platform config directory, built-in defaults (FR-CFG-1).

The project file is read only after the user marks the project as trusted. An untrusted project file cannot enable extensions or change permission defaults (FR-PERM-9).

Environment variables use the `LCA_` prefix with the key uppercased and dots replaced by underscores, so `tool.timeout_seconds` is `LCA_TOOL_TIMEOUT_SECONDS`.

## Keys

| Key | Type | Default | Notes |
|---|---|---|---|
| `provider` | string | `openai-compatible` | The active provider extension's name. |
| `model` | string | the provider's own default | The active model identifier. `/model` overrides it for a session. |
| `compaction.threshold` | float 0.0–1.0 | `0.8` | Context-window fraction that triggers compaction (FR-SESS-4). |
| `provider.retry_limit` | integer | `3` | Retry attempts for retryable transport errors, exponential backoff (FR-CORE-6). |
| `tool.timeout_seconds` | integer | `120` | Shell command timeout; the command's process tree is killed on expiry (FR-TOOL-5). |
| `tool.result_limit_bytes` | integer | `65536` | Tool results above this are truncated and marked (FR-TOOL-7). |
| `tool.max_iterations` | integer | `50` | Maximum tool calls within one turn (FR-CORE-9). |
| `cache.noise_floor_tokens` | integer | `1024` | Cache misses below this are not counted (FR-CACHE-3). |
| `extensions.log_limit_bytes` | integer | `4096` | Extension log messages above this are truncated (FR-EXT-10). |
| `update.check` | boolean | `true` interactive, `false` headless | Daily background version check (FR-CFG-6). |
| `ui.color` | `auto` or `never` | `auto` | `never` forces plain text on terminals without color support (FR-UI-5). |
| `permissions.proposals` | table | empty | Project file only. Proposals with no force; see ADR-0006. |

## What does not live here

**Credentials.** Tokens and keys go in the credential store, never in configuration and never in the session log (FR-CFG-5).

**Extension enablement, project trust, and ad hoc grants.** These live in the user grant store, keyed by the canonical project path, not in any file inside the project directory (FR-PERM-8, FR-PERM-19).

**Provider endpoints and keys.** Each provider's base URL and login are set through that provider's own setup or login flow, so the ad hoc `net` grant for a user-chosen host attaches at the moment the host is named (FR-PERM-16).

**Telemetry.** There is none in 1.0 (FR-CFG-3).
