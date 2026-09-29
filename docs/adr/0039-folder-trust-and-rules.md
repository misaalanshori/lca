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
workspace. It refuses command substitution, `cd`, shells and
indirection, privilege and disk tools, egress clients, package
publishes, sensitive environment variables, heredocs, and any path or
redirection target that leaves the workspace. Everything else is allowed
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
