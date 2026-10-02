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

## Annotation — 2026-09-28 (cycle 3, the pre-review audit): the component
indirection was dead code, the line-string model survives as functions

The decision above stands; this records what the implementation settled
where it left the decision's vocabulary behind.

- **"Every component renders to `Vec<String>`" — the line-string model is
  the contract, but there is no `Component` trait carrying it.** The
  ported `Component` trait, pi's `Container`/`LayoutNode` layout trees,
  the normalized mouse dispatch, and the primitive widgets (`Text`,
  `SelectList`, `ScrollView`, `Loader`, `ImagePlaceholder`) had no
  caller: the interface composes line strings directly and renders the
  protocol `WidgetTree` through `lca-ui`'s `widget_lines`, never through
  engine components. They were deleted (`3ff62fb`, `67e08fd`), and the
  widgets that remain are real ones with callers: editor, markdown,
  autocomplete, image. Rendering to plain line strings is unchanged; it
  is now a property of functions (`transcript::render`, `editor::render`,
  …) rather than of a trait object.
- **"the primitive widgets" in the crate description** is therefore no
  longer part of `lca-tui`. The crate doc and the module doc say what it
  actually holds.

## Annotation — 2026-09-30 (TUI cycle 7, R2): the default flipped to the
main screen; the alt screen is the opt-in

The decision above stands, with one correction the owner's real-terminal
report forced. It says "the **alt-screen** renderer (the default) and the
**main-screen** differential scrollback renderer, switchable at runtime".
The alt screen had been the default since the renovation, and that default
was wrong for the product.

**What broke.** The alt screen enables mouse tracking
(`?1000h?1002h?1003h?1006h`; multiplexer-aware) so the application can own
selection and click-to-open links. A real terminal that has handed clicks
to the application also stops doing its own text selection, right-click
paste, and Ctrl+V — the owner's report: "blocks selection/left and right
clicks", "can't right click and ctrl-v". Every native affordance died the
moment the interface opened.

**pi does not make this trade by default.** `tui-main-screen.ts` never
touches the mouse; only pi's opt-in alt screen captures it, and pi's own
CLI defaults to `--tui-mode regular` (verified against pi 0.87.1's
`--help`). The main-screen renderer's whole reason to exist is that the
transcript lands in the terminal's real scrollback, where the terminal's
selection and links work natively.

**The correction.** The **main-screen renderer is the default**; the
alt-screen renderer is the `/fullscreen` opt-in, with app-owned selection
and the mouse capture that implies. FR-UI-21's runtime toggle is unchanged.
A persisted `ui.json` still wins, so an existing explicit pick survives;
only the no-file default changed. The glossary's alt/main entries and
`docs/configuration.md`'s `ui.fullscreen` row were updated to match.

**Guards.** `main_screen::tests::main_screen_never_enables_mouse_tracking`
asserts the absence of every mouse-enable sequence; `alt_screen::tests::
alt_screen_emits_exactly_the_pi_mouse_sequences` asserts the exact enable
set on entry and the disable set on exit; `lca-cli`'s hooks test asserts
the fresh default and the persisted override.

## Annotation — 2026-10-02 (UI/UX Phase 2, S1): the default flips to the main screen with pi's scrollback contract

**What changed the answer.** The 2026-10-01 flip back to alt-screen (fullscreen) rested on LCA's earlier broken scrollback mode, which truncated document lines before rendering and never pushed lines into the terminal scrollback via genuine incremental appends. S1 ported pi's `tui-main-screen.ts` contract verbatim: bottom-anchored incremental append, `appendStart` newline rule (`\r\n`), viewport-top tracking across frames, and height/width full-render clearing. Under this contract, the terminal's real scrollback buffer retains all historical turns, and the terminal's native selection, scrollbar, and right-click paste work by construction.

**The correction.** The **main-screen renderer (terminal scrollback) is the default** (`ui.fullscreen = false`). The alt-screen renderer is the opt-in (`ui.fullscreen = true` or `/fullscreen`), retaining app-owned text selection and mouse capture for users who want a self-contained fullscreen view. A persisted `ui.json` still wins over the default.

**Honest names.** Commands, help notices, and settings refer honestly to "terminal scrollback" vs "app-owned screen (fullscreen)", mapping directly to what each mode provides.


## Annotation — 2026-10-01 (the color cycle, R1-R3): the renderers spend the role vocabulary

**What changed the answer.** Cycle 4 gave `lca-ui` pi's ~50-role theme table and cycle 8 gave the renderers their structure, but the renderers kept painting with a handful of accessor styles: the background roles, the `syntax*` classes, and half the vocabulary had no consumer, and the transcript read flat next to pi. This annotation records three changes *inside* the decision this ADR already made - no new crate, no new rendering model.

**The style layer now matches pi's `fg()`/`bg()`.** A style function opens its color (and decoration) and closes only what it opened: `ESC[39m` for a foreground, `ESC[49m` for a background, the decoration's own close code. The previous `ESC[0m` made a styled run inside another styled run blank the outer one, which is exactly what a background band needs not to do. Dialog rows keep their explicit whole-row `SEGMENT_RESET` painting (FR-UI-23), so the cycle-8 bleed fix is untouched; only the styles those rows *contain* changed.

**The renderers spend the vocabulary.** User messages render as a full-width `userMessageBg` band, tool cards carry `toolPendingBg`/`toolSuccessBg`/`toolErrorBg` by state, `[type]` headers take `customMessageBg` + `customMessageLabel`, a picker's selected row is `selectedBg` on accent text, the footer colors the thinking level and thresholds the context share, and fenced code blocks paint the nine `syntax*` classes through a `MarkdownTheme::highlight` hook - pi's `theme.highlightCode` seam, which lives in the theme, not in the markdown widget, exactly as it does over there. The grammars behind that hook are hand-written token classes for the languages a transcript names: ADR-0036's no-new-dependency rule still holds, and an unknown fence falls back to `mdCodeBlock` the way pi does. Language auto-detection stays off for pi's stated reason.

**The separator is pi's spinner-in-the-border.** The dock's divider row (`separator.rs`) is idle dashes in the **thinking-level border color** - which is `thinkingOff`, the same darkGray as `borderMuted`, when no level is set, and exactly pi's `editor.borderColor = getThinkingBorderColor(level)` rule - and carries `── ⠴ Working ─────…` or `Retrying (n/m) in Ns…` while the turn runs, animated on `Chat::tick` at pi's 80 ms cadence. The engine contract did not move: it is still one line string per row, and an idle interface still never repaints.

**One defect fixed on the way.** The transcript's per-entry render cache was keyed by width only, so a theme change served rows painted in the old palette. It is now dropped on `/theme`, its live preview, and a detected-scheme swap.

**What to watch.** The hand-written grammars are the deliberate ceiling: a fence naming a language they do not know gets pi's *unknown-language* fallback (whole block in `mdCodeBlock`) rather than a wrong parse, and the fallback is tested. If a real miss costs more than the size budget, `syntect` is the upgrade path - it would be a new dependency, so it needs this record's argument, not just a crate addition.
