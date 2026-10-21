# Terminal setup

Most terminals work with LCA with no setup. Use this page when links,
images, colors, keys, or scrolling misbehave. LCA's terminal claims
are pinned by tmux-driven tests (the testing plan's terminal
sections), so a symptom here that survives the fixes below is a bug
worth reporting, not a configuration you missed.

## Pick a renderer first

Two renderers, one toggle. The scrollback renderer (the default)
appends to the real terminal scrollback: native selection,
scrollbar, right-click paste, and `Ctrl+V` work by construction. The
alt-screen renderer (`/fullscreen`) pins a dock — separator, notice,
queue, editor, footer — over a scrolling transcript with its own
scrollbar; `End` jumps back to the live bottom. If mouse or scroll
acts strangely, say which renderer you are in; they own different
halves of the input stack.

## Links, images, colors, progress

LCA detects these automatically. Override only when detection is
wrong (a proxy or multiplexer in the path), and only toward
something the whole path supports — forcing a capability the
terminal lacks corrupts rendering.

| Feature | Auto behavior | Override |
|---|---|---|
| Hyperlinks (OSC 8) | On in known-capable terminals, off under `screen`, follows the tmux client's own features | `LCA_HYPERLINKS=1` forces on, `=0` forces the `text (url)` fallback |
| Inline images | Kitty and iTerm2 protocols detected | `terminal.show_images=false` drops image rows; `terminal.image_width_cells` caps the width (default 60); `images.auto_resize=false` sends originals |
| Truecolor | Detected | `ui.color=never` forces plain text |
| Taskbar progress (OSC 9;4) | On while the agent is busy — a turn, a background `/compact`, or a `!`/`!!` run — refreshed once a second (gh #369) | `terminal.show_progress=false` silences it |
| Tab status (OSC 7501, gh #213) | On after the terminal answers the support query; off under `screen` | `LCA_PROGRAM_STATUS=1` forces reports, `=0` silences them |

`terminal.hyperlinks`, `terminal.images`, and `terminal.true_color`
exist as accepted-but-inert keys (pi's names, gh #82): detection
lives below config's reach, so they parse and do nothing. The
environment variable (hyperlinks) and the keys above are the real
controls.

### Tab status (gh #213)

LCA reports its lifecycle over the [Program Status Protocol (OSC 7501)](https://www.superlogical.com/rex/docs/build/program-status) — `working` while the agent is busy, `blocked` while a modal waits for you, `done` / `error` when the turn settles, `idle` while the editor waits, `clear` on shutdown — so tab bars and dashboards can badge it. Reports carry only the state and `app=lca`, never prompt text or output. Unknown 7501 traffic arriving on stdin is swallowed, never typed.

`working` covers more than a turn (gh #369): a background `/compact` summarizing context and an inline `!`/`!!` shell run are both work you started and are waiting on, so they badge `working` too. `blocked` likewise covers every modal that waits on you — permission, secret, picker, grant, provider-switch confirm, dialog, background login, and the `/extensions` install-consent prompt; the list is one table in the source (`BLOCKED_MODALS`) rather than a hand-kept condition, so a new modal cannot be forgotten silently.

States are emitted on transition only, never per frame, and a repeated frame writes nothing. A suspend/resume (`$EDITOR` via `Ctrl+X Ctrl+E`) clears the badge on `stop` and **restores** the remembered state on `start`, so returning from the editor does not leave the tab blank for the rest of the session.

### Taskbar progress keepalive (gh #369)

`OSC 9;4;3` is re-emitted once a second while the indicator is active. Terminal emulators put a watchdog on the sequence (~10–15 s) so a crashed CLI does not spin a taskbar forever; a single write at turn start therefore lost the indicator on any turn longer than that, which is most of them. The refresh comes from the interface's existing 50 ms tick — no new thread — and stops with the turn: once the indicator is off nothing is written again, because an idle interface never repaints. `set_progress(false)` and `stop()` write the `OSC 9;4;0` clear immediately.

## Keys and pasting

LCA reads extended keys where the terminal sends them and degrades
where it does not: `Ctrl+J` is the newline alternative when
`Shift+Enter` never arrives, `Alt+P` cycles models back where
`Ctrl+Shift+P` is eaten. Bracketed pastes land intact (tmux's CSI-u
re-encoding of control bytes inside a paste is decoded, so a pasted
newline is a newline, not a submit). Remap anything in
`~/.lca/keybindings.toml` (`/hotkeys` shows the effective map,
`/reload` applies an edit without restarting — except an already-created
prompt editor, which keeps its startup key map until you restart; see
[Configuration](configuration.md#key-bindings) and gh #332).

## tmux, multiplexers, and SSH

Running inside tmux is supported and tested: mouse clicks on tool
cards, bracketed paste, and the hyperlink ladder (which follows the
tmux client's own features) all work through it. Over SSH, prefer a
modern terminal on the local end; the capability detection runs
against the far end's report, so a dumb `TERM` export on the remote
side is the usual culprit when colors or keys degrade.

## Windows and ConPTY

On Windows, shell commands reach the child through temp script
files, never argv (quoting corrupts `"` otherwise); Git Bash is
found by known install paths, then pwsh, powershell, cmd
([platform notes](platform-notes.md)). If your terminal is ConPTY,
read the platform notes' ConPTY section first — scrollback and path
quirks there are terminal bugs LCA works around, documented per
case.

## When it still misbehaves

Run with `--verbose` (tracing to stderr) or read
`~/.lca/logs/lca.log` (bounded, rotates at 1 MiB). The log level
follows `LCA_LOG`, then `RUST_LOG`, then `warn`. Include the
renderer, the terminal (and tmux/client versions), and the relevant
log lines in the report.
