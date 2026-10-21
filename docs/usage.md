# Using LCA in the terminal

Run `lca` from the folder you want to work in. That folder is the
workspace: file tools resolve inside it, sessions group under it, and
its `.lca/` directory holds project settings once you trust it. New
here? Install first ([installation](installation.md)), then come back.

## First session

1. `lca` opens on an empty session with `no model` in the status line.
2. `/login` opens the endpoint picker — 19 presets ship (OpenAI,
   OpenRouter, DeepSeek, OpenCode Go, …) plus `Custom endpoint…` for
   any OpenAI-compatible base URL. Pick one, paste the API key into
   the masked field. The key lands in that extension's own credential
   namespace, never in the session log.
3. If the endpoint is not covered by the extension's manifest, a
   consent prompt names the exact host and asks once; allowing it
   persists an ad hoc grant for the project.
4. `/model` filters the live model list from that endpoint; pick one.
   `Ctrl+P` cycles enabled models (`Ctrl+Shift+P` or `Alt+P` cycles
   back), `Ctrl+S` saves the highlighted one as the default every new
   session starts on.
5. Type a prompt and press `Enter`. Answers stream as they arrive;
   `Ctrl+C` cancels the turn and keeps everything already written.

## Send a prompt

The editor holds one message. `Enter` sends it; `Shift+Enter` inserts
a newline (or `Ctrl+J` where the terminal eats modified keys).
`Tab` completes file paths. `!command` runs a shell command and
sends its output with your message; `!!command` runs one the model
never sees. Paste an image and it rides the turn when the model
takes vision (`/attach` does the same from a path).

## Follow the work

Every tool call lands on the transcript as its own card with its
arguments, result, and cost. Click a card to collapse it; `Alt+T`
toggles how much of a reasoning run shows (snippet, full, hidden).
The footer tracks the folder, session, model, context share, and
accumulated cost. The startup header names what loaded — context
paths, extension/skill counts — so a missing resource is visible,
not mysterious.

LCA does not ask before every tool call. Watch the transcript,
review diffs, and keep credentials out of the room (see
[Security](security.md)).

## Steer a running turn

You can type while the model works. `Enter` steers the current task
once the turn's tool calls finish; `Alt+Enter` queues a follow-up
that runs after the task. `Esc` stops the turn. `Esc Esc` with an
empty editor opens the branch tree (or fork picker, or nothing —
`/settings` decides under `ui.double_escape_action`).

## Change the model or settings

Type `/` to search every command; `/help` lists them all with one
line each. The everyday set:

- `/model`, `/thinking` — model and reasoning level.
- `/login`, `/logout`, `/usage` — provider access and spend.
- `/compact` — shrink the history the model sees.
- `/tree`, `/fork`, `/clone` — explore without losing the thread.
- `/resume`, `/name`, `/session` — find and title sessions.
- `/trust`, `/grants`, `/permissions` — what the project may do.
- `/reload` — re-read settings, extensions, themes, keybindings.
- `/settings`, `/theme`, `/fullscreen` — the interface itself.
- `/export`, `/attach`, `/mcp`, `/skill` — artifacts and resources.

Skills and extensions add their own `/`-commands to the same menu.

## Sessions, branches, and starting over

Every session persists as an append-only log; `/resume` reopens any
of them, `/name` titles the current one, `/session` shows its file,
id, message count, tokens, and cost. `/fork` clones the tip into a
new session; `/tree` walks the branch history; `/compact` replaces
old history with a summary so a long session stays affordable.

In `/tree`, a session with no forks reads as a flat list - rows indent
only where the history actually forks, so a hundred-turn conversation
still fits a narrow pane. The branch you are on sorts first, `f` cycles
which kinds of row show, `←`/`→` fold and unfold, and `Alt+←`/`Alt+→`
fold a branch or jump to the next one. From
outside, `lca resume` lists the project's sessions, and
`lca -c -p "continue"` appends a headless turn to the latest one
(see [CLI integration](cli-integration.md)).

## Adjust the interface

`/settings` edits the user-facing keys with the winning source
shown per row. `/fullscreen` flips between the scrollback renderer
(the default: the terminal owns scroll and selection) and the
alt-screen renderer (the app owns them). `/theme` picks the palette.
`~/.lca/keybindings.toml` remaps keys; `/hotkeys` shows the
effective map. When the terminal misbehaves, see
[Terminal setup](terminal-setup.md).
