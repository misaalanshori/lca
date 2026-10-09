# 0046. Sessions grow a tree on top of forks, not instead of them

Status: accepted (2026-10-10).

## Context

Pi stores a session as one JSONL file whose entries form an
`id`/`parentId` tree (`docs/session-format.md`): branching sets a leaf
pointer, labels bookmark entries, branch summaries carry abandoned-path
context forward, and `/tree` navigates in place. LCA stores a session
as one JSONL file per session directory, and a branch is a child
directory with a `fork-point` record (`docs/session-log-format.md`,
"Fork"). `/tree` browses fork directories; there are no labels, no
summaries, no in-place rewind (gh #37).

The question is fork-chain vs in-file tree: migrate the log to
`id`/`parentId` linkage, or keep forks and add tree semantics on top.

## Decision

**Keep forks; layer the tree on top.** The fork machinery is shipped,
tested, and load-bearing far beyond branching: attachment sharing walks
the fork chain, `lca session gc` mark-and-sweeps the fork tree, export
resolves ancestors, and every existing session on disk is a fork chain.
An in-file migration would rewrite or dual-read all of that, under a
freeze that forbids breaking record shapes, for a payoff (one file per
branch family) that users never asked for. So:

- Forks stay the branch container. Navigating to an earlier message and
  continuing means fork-at-record plus switch, until the tree phase says
  otherwise. The "leaf" today is the tip of the live fork.
- Labels work on record ids under either model, so they come first (gh
  #47 already defined the `label` record; this cycle wires writers and
  surfaces). A label names a record, never a session: it survives
  whatever the tree phase later decides.
- A later phase adds an additive optional `parent` on records plus a
  leaf pointer, enabling one-log branch/resume (the
  `branching_at_a_record_and_resuming_the_branch_keeps_one_log`
  acceptance), then tree navigation UI, branch summaries (pi's
  `branch_summary` shape: `fromId` + summary at the navigation point),
  and rename surfaces. Old fork-chain sessions keep loading unchanged;
  no migration tool is built (the #98 touchpoint stays a note).

What pi behaviors we adopt regardless: the `label` vocabulary and its
latest-wins semantics, the `branch_summary` shape when summaries land,
and the leaf-ancestry rule for context assembly when the tree lands.

## Alternatives considered

**Migrate to pi's single-file tree now.** Rejected: every existing log
would need a rewrite or a dual reader, GC/attach/export would need
reworking against references instead of directories, and the freeze
turns every record-shape change into a minor-version decision. The
benefit is one file per family; the cost is the durability story
(`docs/session-log-format.md` is stricter than it looks necessary on
purpose). Revisited only if fork chains prove unmaintainable in
practice — the revisit signal is named below, not assumed.

**Labels keyed by session.** Rejected: bookmarks mark entries ("the
auth-refactor turn"), and sessions already have titles. Keying by record
id keeps labels valid across forks and across the future tree.

## Consequences

- New `label` writers append; latest wins per target; absent clears.
  Assembly ignores labels; export carries them (they are conversation
  shaping, not audit).
- `/jump` is fork-at-record plus switch with the label named in the
  notice — honest about the model, no fake in-place semantics.
- Trust flags (`-a`/`-na`, `trust.default_project`) ride ADR-0039, not
  this record: process-scoped session trust/distrust, refused loudly in
  project files.

## Phase cut lines

- Phase 1 (this): this ADR, label add/list/jump on record ids, trust
  flags.
- Phase 2: additive `parent` linkage, resume-from-record in one log,
  branch summaries.
- Phase 3: tree navigation UI, rename surfaces, tree-aware export.
- Never in this epic: migration machinery (a #98-adjacent note only),
  stream observation (came with #45, stays deferred per gh #80).

## Addendum (2026-10-10, gh #37 phase 2)

Phase 2 landed three decided details worth pinning:

- **No stored leaf pointer.** The walk starts at the log tip and a
  branch jump is transparent, which covers every navigation
  (including re-navigation with nothing appended yet: the tip *is*
  the jump). One less piece of mutable state; the log stays the sole
  authority.
- **Forks inherit chains, not prefixes.** Splicing the taken
  record's ancestry keeps an abandoned path out of the child; on
  unlinked parents the walk degrades to the old prefix exactly.
- **GC and labels see the file.** Reachability walks audit minus
  compaction suppression (abandoned attachments survive, orphans do
  not); bookmarks resolve file-globally. Display, transcript, and
  model context see the live chain only.

## Revisit conditions

- Fork chains produce a real defect class (broken-chain truncation
  reports, GC misses, attach misses) at a rate that in-file ancestry
  would structurally prevent. Anecdotes do not qualify; the
  regressions directory is the counter.
- A tree-phase prototype shows the leaf pointer cannot live alongside
  fork directories without dual readers everywhere. Then the migration
  gets priced, not assumed.
