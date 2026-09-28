# ADR-0036: Dependencies for the pi TUI port (`lca-tui`)

Status: accepted.

Date: 2026-09-27.

## Context

The owner's brief (`LCA-PROMPT-TUI-20260927-1807.md`) replaces the
ratatui/crossterm interface with a Rust port of pi's `packages/tui` engine
and widgets, then rebuilds the agent interface on it (`lca-ui`). A port of
this size touches text measurement, grapheme handling, and a raw terminal
backend, and the SRDD's dependency rule requires each addition to state
what it does, why writing it is worse, and what it costs.

The brief sanctions exactly two new crates, `unicode-width` and
`unicode-segmentation`, under one ADR, and argues the remaining low-level
needs are already in the workspace graph through `lca-tools`.

## Decision

Add four direct dependencies to `lca-tui`, all already resolved in
`Cargo.lock`, so the workspace's transitive supply-chain footprint does
not change:

- `unicode-width` (0.2) — East Asian width and ambiguous-width classes.
  pi's `utils.ts` measures width with `string-width`/`get-east-asian-width`
  and layers terminal-bug corrections on top (Thai/Lao decomposition,
  regional indicators pinned at 2, tab = 3). Writing the Unicode width
  tables by hand is thousands of generated lines and a guaranteed source
  of drift against the Unicode release the terminal actually follows.
- `unicode-segmentation` (1.13) — grapheme-cluster iteration, the Rust
  equivalent of the `Intl.Segmenter` use in pi's editor (paste markers and
  cursor motion treat a marker as one grapheme). Same argument: the
  segmentation tables are not something to hand-maintain.
- `libc` (0.2, `cfg(unix)`) — termios raw mode, `TIOCGWINSZ`, `read(2)`,
  `isatty`. Already a direct dependency of `lca-tools` for the pty
  capability with the same justification.
- `windows-sys` (0.61, `cfg(windows)`) — console mode
  (`ENABLE_VIRTUAL_TERMINAL_INPUT`), screen-buffer info, and `ReadFile` on
  the stdin handle for the Windows terminal backend. Already a direct
  dependency of `lca-tools` for Job Objects at this version.

No other dependency is added for Group I. In particular, the port does not
depend on `crossterm`'s event parser; pi's dialect tables are the ground
truth and are parsed from raw bytes. `crossterm`/`ratatui` remain in
`Cargo.toml` only until Group II retires the legacy renderer (see the
consequences).

## Alternatives considered

Keep the low-level work on `crossterm` (already a dependency) and only
parse keys ourselves. Rejected: crossterm's Windows event reader is the
only portable way it surfaces input, and once it consumes stdin the raw
byte stream the Kitty/modifyOtherKeys dialect tables need is gone. Reading
raw bytes ourselves on Unix and using `windows-sys` on Windows keeps one
input model instead of two.

Hand-roll Unicode width and grapheme segmentation. Rejected: both tables
are machine-generated from the Unicode database and change with Unicode
releases; a hand port would drift silently and the bugs would surface as
misaligned tables and broken cursor motion, which are exactly the failures
this port exists to fix.

Add a general-purpose TUI framework (`ratatui` replacement) instead of a
direct port. Rejected by the brief and by the evidence: pi's selection
subsystem, differential scrollback renderer, and editing model are the
specific behaviors the owner's issues need, and a framework would not
supply them.

## Consequences

`lca-tui`'s dependency set grows by two new crates (both new to the
workspace, both at versions already in `Cargo.lock` transitively) and two
promotions of existing transitive dependencies to direct ones. The binary
size gate (NFR-1/NFR-2) is re-measured after Group I; the width/segmentation
tables are small and the two low-level crates are already linked by
`lca-tools`.

During Group I the legacy ratatui renderer stays compiled so the shipped
`lca` binary keeps working; `crossterm` and `ratatui` leave `Cargo.toml`
when `lca-ui` replaces the legacy path in Group II.

## Revisit conditions

A binary-size regression that cannot be attributed to the port's own code;
or a Unicode-width disagreement between `unicode-width` and a terminal
that makes the width corrections in `utils.ts` insufficient. The width
correction layer is deliberately on top of `unicode-width`, not instead of
it, so it can absorb such a case without replacing the dependency.

## Addendum (cycle 3, R2): `regex` for the resume search grammar

`lca-ui`'s `/resume` list carries pi's session-search grammar
(`session-selector-search.ts`): `re:<pattern>` is a case-insensitive
regex, `"quoted phrase"` is an exact substring, and bare terms are an
AND of case-insensitive substrings. The regex arm needs a regex engine,
and `regex` is already a direct dependency of `lca-tools` (the grep tool)
with the same justification: writing one is a multi-year project and the
workspace's supply-chain footprint does not change (it is already
resolved in `Cargo.lock`, and `lca-ui` already depends on `lca-tools`).
The alternative — routing every keystroke's search through a host
callback — puts the grammar in the host instead of the interface that
owns the picker, which is the wrong layer.
