# 0037. The TUI is ours: two crates and the line-string engine

Status: accepted (2026-09-28). Implementation lands in the TUI renovation
phases; where a statement below outpaces the code, the code converges
toward it, never the other way.

## Context

The first releases rendered through ratatui over crossterm. Hands-on use
paid the cost of that abstraction: the transcript fought a widget model
built for dashboards, text selection depended on the terminal's own
selection (and its clipboard honesty), and the interface could not grow
the behaviors a coding agent needs — streaming-tolerant markdown, a real
prompt editor, selection that masks what the renderer masks. The SRDD's
dependency table always named the alternative: a renderer written in the
project. The design source chosen for it is pi's TUI (`packages/tui` of
`github.com/badlogic/pi-mono`, MIT, Mario Zechner), reverse-engineered
file by file before any porting began.

## Decision

**Two crates, one hard boundary.** `lca-tui` is the terminal engine and
widget library: terminal I/O and protocol negotiation, input parsing and
keybindings, the two screen renderers and the selection subsystem, the
editor, markdown, autocomplete, and the primitive widgets. `lca-ui` is
the agent interface: transcript, chrome, selectors, theme, modals. The
boundary rule is checked mechanically — `rg 'lca_core|lca_protocol|
lca_session|lca_provider|lca_config' crates/lca-tui/src` is empty — and
anything that needs agent data is a wrapper in `lca-ui`, never a leak
into the engine.

**Line-string rendering.** Every component renders to `Vec<String>` at a
width; a renderer diffs that against what is on screen. Two renderers
exist: the **alt-screen** renderer (the default) and the **main-screen**
differential scrollback renderer, switchable at runtime. Rendering to
plain line strings is what makes the engine unit-testable without a
terminal.

**The renderer owns text selection.** Drag, word, and line granularity;
copy through a verified native clipboard when the host provides one,
else OSC 52 (written unverified - the engine has no native clipboard to
confirm it). A drag on a viewport edge auto-scrolls and extends the
selection, and a click on an OSC-8 link hands the URL to the host
(`UiHooks.open_url`). This is
deliberate: selection goes through the same masking the transcript does,
and the terminal's own selection is not the product's contract.

**One key vocabulary.** Input is parsed from raw bytes by the engine —
Kitty keyboard protocol negotiation, xterm `modifyOtherKeys` fallback,
legacy sequences, bracketed paste — into a single key type that flows
end-to-end to the editor and the keybinding registry. Translating down
to a coarser event type mid-pipeline (as an interim step once did)
discards exactly the disambiguation the parsing exists to preserve.
There is one editor implementation.

**ratatui is retired.** crossterm remains only where non-interactive code
needs terminal input; the interactive path never translates through it.

## Consequences

- The engine is the most testable surface in the workspace (pure string
  transforms over a fake terminal) and the future port target for the
  web host (NFR-11): the engine crate carries no agent or OS coupling
  beyond one documented `unsafe` module (`engine/sys.rs`, the
  termios/`SetConsoleMode` exemption mirroring `lca-tools/src/pty.rs`).
- Terminal-protocol knowledge concentrates in that engine; its in-tree
  home for documentation is `docs/platform-notes.md`.
- This is a renovation: phases may break the tree mid-move. The rule is
  that commits and phase boundaries land green, and real-terminal
  driving (`docs/testing-plan.md` §14) is the proof the interface works
  while the architecture moves.
- Attribution: the design follows pi; the licensing question (NOTICE,
  dual license) remains owner-deferred.
