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
never sees. The card says which one you ran: `!bash …` in the accent
color, `!!bash …` quiet, so the context boundary is visible at a glance
and still readable with color off. Paste an image and it rides the turn
when the model takes vision (`/attach` does the same from a path).

## Follow the work

Every tool call lands on the transcript as its own card with its
arguments, result, and cost. Click a card to collapse it; `Alt+T`
toggles how much of a reasoning run shows (snippet, full, hidden).
The footer tracks the folder, session, model, context share, and
accumulated cost. The startup header names what loaded — context
paths, extension/skill counts — so a missing resource is visible,
not mysterious. You can start typing before the header finishes:
the keyboard is read from the first frame, so what you type is
buffered and lands in the editor in order rather than being lost or
appearing late. `Ctrl+C` during that startup window is honoured too,
so leaving a slow start does not require waiting for it.

LCA does not ask before every tool call. Watch the transcript,
review diffs, and keep credentials out of the room (see
[Security](security.md)).

## Steer a running turn

You can type while the model works. `Enter` steers the current task
once the turn's tool calls finish; `Alt+Enter` queues a follow-up
that runs after the task. `Esc` stops the turn. `Esc Esc` with an
empty editor opens the branch tree (or fork picker, or nothing —
`/settings` decides under `ui.double_escape_action`).

Queued messages sit above the editor as `Steering:` and `Follow-up:`
lines. Stopping the turn with `Esc` puts the whole queue back in the
editor, ahead of anything you typed meanwhile, so nothing is lost and
nothing is sent without you. `Alt+Up` does the same on demand while
the turn keeps running. Typing during `/compact` queues the message
for the moment compaction finishes instead of being refused.

## Run a shell command with `!`

`!<command>` runs it and streams the output into a card in the transcript; `!!`
does the same but keeps the command and its output out of the model's context.
Both are cancellable with `Esc` while they run.

Some commands need a real terminal rather than a pipe: an editor, a pager, an
interactive `git` subcommand, a REPL, anything fullscreen. Those would either
refuse (`vim: output is not to a terminal`) or stop and wait for keys that never
arrive - a hang that looks like the agent is still working. `!` recognises them
and **hands the terminal over**: the interface steps aside, the command runs with
its own stdio, and the interface comes back with a card showing the exit code.

```
!vim notes.txt        # the editor opens; :q returns you here
!git add -p           # hunk-by-hunk, as in a shell
!lazygit              # fullscreen
!git log | less       # a pager tail counts, so this opens `less`
```

**Detection is a short, explicit list, and it errs towards staying piped.** A
false positive - running `git log` fullscreen when you wanted its output in the
transcript - breaks the feature, so only programs whose *primary* mode is the
terminal are matched, and only as the program word: `grep vim notes.md` is a
search and stays piped.

When the list does not know your program, or a wrapper hides it, `!i` forces the
handoff:

```
!i ./my-curses-tool    # runs with the terminal handed over
```

Inside a handed-over command `Ctrl+C` reaches the command, not the agent - it
owns the terminal while it runs. If it ignores signals, kill it from another
shell (`kill -9 -<pgid>` for a whole process group); the agent cannot offer a key
of its own while the child holds the terminal.

## Work in more than one tab

`Ctrl+T` opens another conversation in the same folder, and each tab
keeps its own session, its own draft, and its own place in the scroll.
`Alt+1`..`Alt+9` jump straight to a tab, `Ctrl+Tab`/`Shift+Ctrl+Tab`
cycle, and `Ctrl+W` closes one (the session stays on disk; only the
unsent draft goes).

**A tab switch does not stop a running turn.** Start a long
`cargo test`, switch to another tab, and keep working: the first tab
keeps streaming, its item in the tab bar pulses `◌`, and when it
settles it shows an unread `•` until you look at it again. Switching
back finds the turn exactly where it was.

Three things follow from that, and they are deliberate:

- `Ctrl+C` cancels the turn of the tab you are **looking at**. A turn
  running in a background tab is not cancelled by a keystroke you
  cannot see the effect of — switch to that tab first, then `Ctrl+C`.
- Closing a tab whose own turn is still running refuses and says so:
  the stream is landing in the row about to disappear, so the work
  would be lost. Cancel it or switch away first.
- Opening a new tab (`Ctrl+T`) also waits for the turn to finish.
  Creating a session makes it the host's current one, and the running
  turn is still writing to the one it started in; switching tabs is
  deferred for exactly that reason, but there is nothing to defer a
  brand-new session to. Switch tabs to work elsewhere in the meantime.

## Change the model or settings

Type `/` to search every command; `/help` lists them, one line each. The
everyday set:

- `/model`, `/thinking` — model and reasoning level.
- `/login`, `/logout`, `/usage` — provider access and spend.
- `/compact` — shrink the history the model sees.
- `/tree`, `/fork`, `/clone` — explore without losing the thread.
- `/resume`, `/name`, `/session` — find and title sessions.
- `/trust`, `/grants`, `/permissions` — what the project may do (see
  [below](#permission-rules) for how a rule is written and what it matches).
- `/reload` — re-read settings, extensions, themes, keys (restart for
  a changed `tui.editor.*` or `tui.input.*` binding; see
  [Configuration](configuration.md#key-bindings)).
- `/settings`, `/theme`, `/fullscreen` — the interface itself.
- `/attach`, `/paste` — stage an image for the next message.
- `/mcp`, `/ext`, `/skill` — servers, extensions, and skills.

Skills and extensions add their own `/`-commands to the same menu.

### Bookmarks and book copies

- `/label [message-index] <name>` — bookmark a message; with no index,
  the latest one.
- `/labels` — list every bookmark, oldest name first.
- `/jump <name>` — branch at a bookmark and switch to the copy.

### Rotate the model quickly

- `/scoped-models` — choose which models `Ctrl+P` cycles through.

### The full command surface

This table is every command the interface dispatches, and `/help` lists
the same set with the same one-line text — the two cannot drift apart
(gh #443 added the guard that proves it). Skills and extensions add
their own `/`-commands to the same menu.

| Command | What it does |
| --- | --- |
| `/attach <path>` | attach an image file to the next message |
| `/bug` | write a local diagnostic bundle (scrubbed copy, reviewed before writing) |
| `/changelog` | show the latest released changes |
| `/clone [name]` | duplicate this session at its tip and switch |
| `/compact` | summarize the session to free context |
| `/exit` | leave the interface (`/quit` is the same) |
| `/export` | write the session out (add html for the transcript page) |
| `/ext` | manage extensions (enable/disable, compaction strategy) |
| `/extensions` | open the extension hub |
| `/fork [n]` | fork a branch at a message (picker, or the nth) |
| `/fullscreen` | toggle terminal scrollback and app-owned screen |
| `/grants` | review this project's grants and rules |
| `/help` | list commands and keys |
| `/hotkeys` | list every key binding |
| `/import` | import JSONL lines as a new session |
| `/jump <name>` | branch at a bookmark and switch |
| `/label [n] <name>` | bookmark a message |
| `/labels` | list every bookmark |
| `/login` | sign in to a provider |
| `/logout` | clear the provider's stored key |
| `/mcp` | manage MCP servers (status, reconnect, enable, exposure, login) |
| `/model` | list or switch the session's model |
| `/name <name>` | rename this session (`/rename` is the same) |
| `/paste` | paste the clipboard image to the next message |
| `/permissions` | manage allow/deny rules (session, project, global) |
| `/persona` | apply a named bundle of model, thinking, tools, and instructions |

### Permission rules

`/permissions deny read muffin-recipe.txt` refuses any read of that file, and
`/permissions allow write out/*.txt` allows writes under `out/`. The shape is:

```text
/permissions                            list the rules
/permissions [session|project|global] allow|deny <rule>
/permissions clear                      drop this session's rules
```

The scope defaults to **project**; `global` is your default across every
project, `session` vanishes when the interface exits.

A rule is either **verb-qualified** — `read <path>` or `write <path>` — or a
**bare glob** for anything else (`git push*` for shell commands, `example.com`
for a host). The verb decides which kind of action the rule can match, and the
confirmation line always states the pattern that will actually be compared, plus
what you typed when the two differ:

```text
deny rule `*muffin-recipe.txt` for read actions (from `read muffin-recipe.txt`)
```

A relative path compiles to a pattern that spans directories (`*` + the path you
gave), because actions carry absolute paths. That means a rule written as
`read muffin-recipe.txt` keeps working from a subdirectory and after the checkout
moves; it also means it matches that same path tail anywhere on disk, so prefer an
absolute path when you mean exactly one file. A bare glob is stored exactly as
typed, so `*muffin-recipe.txt` keeps its meaning if you already learned it.

Two things are refused rather than stored, because a rule that cannot match
anything is worse than no rule — the whole reason this is documented:

- a word one keystroke from a verb (`raed secrets.txt` — did you mean `read`?),
  rather than silently becoming a bare glob that matches no read;
- a path of only wildcards (`read *`), which would be every file on the machine.

Deny beats allow at any scope, and a deny never prompts.
| `/reload` | re-run discovery without restarting |
| `/rename <name>` | rename this session (`/name` is pi's spelling) |
| `/resume` | search and reopen a session |
| `/scoped-models` | choose the quick-cycle rotation (`Ctrl+P`) |
| `/session` | show this session's tokens, cache, and cost |
| `/settings` | show the merged configuration and where each value came from |
| `/skill [name]` | list or load a skill |
| `/stats` | show session statistics (a native extension fills this slot) |
| `/theme` | pick a theme with live preview |
| `/thinking` | set the reasoning level |
| `/tree` | browse the session's entry tree and branch in place |
| `/trust` | trust the project folder |
| `/usage` | show the provider's usage, when it has one |

`/bug`, `/export`, and `/import` do the same work as their CLI
spellings (`lca bug`, `lca export`, `lca import`), so either surface
works:

```
/bug                     # write a diagnostic bundle for this project
/export                  # write this session out (/export html for the page)
/import <file.jsonl>     # import JSONL lines as a new session
```

`/persona <name>` applies a bundle defined under `[personas.<name>]` in your
config: a model, a reasoning level, a tool loadout, and optional instructions, in
one step. `/persona` with no argument lists them with what each carries. The whole
bundle is checked against the running session first — if any field does not fit
(the model is not offered, that model does not take that thinking level, a tool is
not registered) **nothing is applied**, because a half-applied persona is a persona
you are not in. See [Personas](configuration.md#personas) for the fields.

`/bug`'s bundle contains the transcript. It is written next to your
project, not uploaded, and sharing it is a deliberate act.

### What `/bug` scrubs, and what it cannot

`/bug` stages a **scrubbed copy** of the session, shows you what is in it, and
writes the zip only when you confirm — `[y]` writes, `[n]` or `Escape` discards
it and removes the staged copy. Nothing is uploaded.

The scrub replaces **secret-shaped strings** in the copy: API keys (`sk-…`,
`sk-ant-…`), GitHub tokens (`ghp_…`, `github_pat_…`), Slack tokens (`xox…`), AWS
key ids (`AKIA…`), Google keys (`AIza…`), bearer tokens, JWTs, OAuth
`code=`/`access_token=` values, and any long opaque value that sits next to a
word like `api_key`, `secret`, `token`, `password`, or `authorization`. Each match
becomes `<redacted>`; the surrounding sentence, the roles, the timestamps and the
tool names are untouched, so the bundle still debugs.

The inspection screen tells you **how many** and **of what kind** were scrubbed —
never the values, not even shortened. That is deliberate: you cannot judge whether
it caught the thing you are worried about if it stays quiet, and a summary that
repeated the secret would be a new leak.

Three honest limits:

- The **scrub is a pattern match, not a proof.** A secret with no recognisable
  shape and no label around it (a bare password inside a sentence) is out of
  reach.
- It **errs toward scrubbing.** A long opaque value beside `token` is replaced
  even if it was an innocent hash. A false redaction costs a maintainer one
  round-trip; a missed one leaks a credential.
- **Your own session log is never touched.** The bundle is a copy, so the
  transcript you resume from still has everything.

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

Quitting the interface prints `resume with: lca --resume <id>` once the session
has been written, so the way back is in the scrollback (gh #403 gap 2). It is
printed only when the command will work: not for a piped or headless run, and not
for a session that never recorded anything — a hint whose command failed would
teach you the tool lies. A session kept outside `~/.lca` is named by path
(`lca --session <dir>`) and shell-quoted, so the line pastes cleanly.

## Adjust the interface

`/settings` edits the user-facing keys with the winning source
shown per row. `/fullscreen` flips between the scrollback renderer
(the default: the terminal owns scroll and selection) and the
alt-screen renderer (the app owns them). `/theme` picks the palette.
`~/.lca/keybindings.toml` remaps keys; `/hotkeys` shows the
effective map. When the terminal misbehaves, see
[Terminal setup](terminal-setup.md).
