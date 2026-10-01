# Configuration reference

Version 0.1, 2026-09-20.

This is the key reference for the merged configuration the requirements document summarizes. It names every key, its type, its default, and which source can set it. `lca config` prints the resolved value of each key and the source that set it, per FR-CFG-2.

## Sources and precedence

Highest first: command line flags, environment variables, the project file at `.lca/config.toml`, the user file in the platform config directory, built-in defaults (FR-CFG-1).

The project file is read only after the user marks the project as trusted. An untrusted project file cannot enable extensions or change permission defaults (FR-PERM-9).

The user file is **`~/.lca/config.toml`** on every platform (R7). Theme files live under `~/.lca/themes/`, the `/fullscreen` toggle writes `~/.lca/ui.json`, and `make`/provider preset overrides live at `~/.lca/provider-presets.toml`. The platform config directory (`$XDG_CONFIG_HOME` or `~/.config` on Linux, `~/Library/Application Support` on macOS, `%APPDATA%` on Windows) is *not* where the agent's own files live; it is what the `fs` capability's `home-config` scope resolves to, for reading another tool's configuration.

Environment variables use the `LCA_` prefix with the key uppercased and dots replaced by underscores, so `tool.timeout_seconds` is `LCA_TOOL_TIMEOUT_SECONDS`.

`LCA_HYPERLINKS` is the one host variable outside that scheme: `1` forces OSC 8 terminal hyperlinks on, `0` forces the `text (url)` fallback, and unset leaves it to the capability ladder (the tmux client's own features, `screen` off, known-capable terminals on, unknown off), which is pi's `PI_HYPERLINKS` shape.

## Keys

| Key | Type | Default | Notes |
|---|---|---|---|
| `provider` | string | `openai-compatible` | The active provider extension's name. |
| `model` | string | the provider's own default | The active model identifier. `/model` overrides it for a session. |
| `compaction.threshold` | float 0.0–1.0 | `0.8` | Context-window fraction that triggers compaction (FR-SESS-4). |
| `provider.retry_limit` | integer | `3` | Retry attempts for retryable transport errors, exponential backoff (FR-CORE-6). |
| `tool.timeout_seconds` | integer | `120` | Shell command timeout; the command's process tree is killed on expiry (FR-TOOL-5). |
| `tool.result_limit_bytes` | integer | `65536` | Tool results above this are truncated and marked (FR-TOOL-7). |
| `tool.max_iterations` | integer | `100` | Maximum tool calls within one turn (FR-CORE-9). A runaway guard, not a work cap; a multi-file build-and-test turn routinely needs more than the old 50. |
| `cache.noise_floor_tokens` | integer | `1024` | Cache misses below this are not counted (FR-CACHE-3). |
| `extensions.log_limit_bytes` | integer | `4096` | Extension log messages above this are truncated (FR-EXT-10). |
| `update.check` | boolean | `true` interactive, `false` headless | Daily background version check (FR-CFG-6). |
| `ui.color` | `auto` or `never` | `auto` | `never` forces plain text on terminals without color support (FR-UI-5). |
| `ui.theme` | string | `auto` | The theme (S5): `auto` follows the detected terminal scheme; `dark`/`light`/`plain` are built in; any other name loads `<config dir>/themes/<name>.toml` (or `<name>.light.toml` / `<name>.dark.toml` for a scheme pair). A theme file names any of the ~50 roles; an invalid file keeps the previous palette and says why. |
| `ui.fullscreen` | boolean | `true` | The `/fullscreen` toggle's persisted choice (FR-UI-21). `true` (the default) is the alt-screen renderer with app-owned scroll and selection; `false` opts into the main-screen scrollback renderer, which leaves the terminal's own selection, right-click paste, and Ctrl+V alone. The default returned to alt-screen on 2026-10-01 after real Windows use showed ConPTY does not reliably restore scrollback in main-screen mode (ADR-0037's second annotation). Written to `<config dir>/ui.json` when toggled, not to the config file, so the runtime toggle never rewrites user config. |
| `shell.tool` | `auto`, `bash`, `pwsh`, `powershell`, or `cmd` | `auto` | The `shell` tool's interpreter (ADR-0041). `auto` on Windows walks Git Bash at its known install locations, then `pwsh.exe`, then `powershell.exe`, then `cmd.exe`; on Unix it is `sh`. A configured tool that cannot be found fails every shell call with the locations searched - never a silent fallback to a different shell. |
| `shell.path` | string | unset | An exact interpreter path, which wins over `shell.tool`. It may name anything, including the WSL stub (`C:\Windows\System32\bash.exe`), because naming a path is deliberate; auto mode skips that stub (it runs in a different filesystem namespace). `/settings` shows the key rows and a `shell.resolved` row naming what the ladder actually picked. |
| `ui.thinking` | `snippet`, `full`, or `hidden` | `snippet` | How much of a reasoning run the transcript shows (R6). `snippet` (the default) renders the first three non-empty lines and then `… +N lines`; `full` renders the whole run; `hidden` keeps pi's single dim line. Ctrl+T overrides the *latest* run in place, and the toggle is per run - older runs keep this setting. Distinct from `thinking` below, which is the effort level the model is asked for. |
| `thinking` | `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max` | unset | The session's reasoning level, pi's vocabulary. Unset means the provider chooses; `/thinking` sets it live. It rides the request extras as `reasoning-effort`, which a provider honors where meaningful. |
| `permissions.mode` | `ask` or `yolo` | `ask` | How permission prompts are answered (ADR-0042). `yolo` answers every prompt "always, for this exact pattern": the pattern is persisted and a `permission` record is written exactly as a human answer would write it. Explicit deny rules still deny. `--yolo` sets it for one process, beating any file. While it is on, the footer carries a `YOLO` line in the error role. Reads outside the workspace never prompt in either mode. |
| `permissions.proposals` | table | empty | Project file only. Proposals with no force; see ADR-0006. |

## Environment

Environment variables are read by the provider extension that speaks the
shape, not by the host, and they take precedence over what the login flow
persisted. The bundled `openai-compatible` provider reads:

| Variable | Meaning |
|---|---|
| `OPENAI_BASE_URL` | The endpoint's base URL. A non-default host needs the ad hoc `net` grant (FR-PERM-16). |
| `OPENAI_API_KEY` | The API key. `OPENCODE_API_KEY` is accepted as a synonym. |
| `OPENAI_MODEL` | The model id. `LCA_MODEL` is accepted as a synonym. |
| `OPENAI_CONTEXT_WINDOW` | The context window, in tokens, when the endpoint does not report one. |
| `OPENAI_PROMPT_CACHE_KEY` | Set to `0` to stop sending `prompt_cache_key`. **Default: on.** The key is the clamped session id, which is OpenAI's native cache-affinity parameter; some strict proxies reject unknown body fields, which is what the opt-out is for. |
| `OPENAI_SUPPORTS_REASONING` | Set to `0` to stop sending `reasoning_effort` when the session's `thinking` level is set. **Default: on.** Same reason as the cache-key opt-out: a strict proxy that rejects unknown body fields. |

## What does not live here

**Credentials.** Tokens and keys go in the credential store, never in configuration and never in the session log (FR-CFG-5).

**Extension enablement, project trust, and ad hoc grants.** These live in the user grant store, keyed by the canonical project path, not in any file inside the project directory (FR-PERM-8, FR-PERM-19).

**Provider endpoints and keys.** Each provider's base URL and login are set through that provider's own setup or login flow, so the ad hoc `net` grant for a user-chosen host attaches at the moment the host is named (FR-PERM-16).

**Named custom endpoints.** The user's own presets live at `<config>/provider-presets.toml`, outside every project directory: a plain TOML list of `[[preset]]` entries (`id`, `name`, `base_url`, `auth = "bearer"|"none"`, `models`), merged into the `/login` picker alongside the extension's own presets (D1's override layer). It is user data in the config directory, not project configuration, so it never travels with a repository.

**Telemetry.** There is none in 1.0 (FR-CFG-3).
