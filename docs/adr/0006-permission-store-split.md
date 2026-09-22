# ADR-0006: Split permission store

Status: accepted.

Date: 2026-09-20.

## Context

The agent remembers approvals. When a user approves a shell command pattern with the always option, that approval persists. The question is where it lives.

A store in the project directory can be committed, reviewed in a pull request, and shared across a team. A new person clones the repository and the agent already knows which build commands are safe.

A store in the project directory also means that pulling a branch can change what the agent is allowed to run, and the change looks like an ordinary configuration diff. In a large merge, nobody reads it. That turns a routine git operation into a privilege escalation path.

A store in the user directory has neither problem and neither benefit. Every developer on a team approves the same commands again, one at a time.

## Decision

Split the two roles. The project file holds proposals. The user store holds grants. Only the user store is consulted when an action runs.

A proposal is a suggestion from the repository. It has no force. Approving a proposal copies it into the user store, once, behind a prompt that shows what is being added.

The user store records the hash of the proposal set it approved. When the project file changes, the hash stops matching, and the agent prompts again showing only the difference rather than the whole file. Without the hash check, an edit to a proposal buried in a large merge lands silently, which is the exact failure the split exists to prevent.

The user store is keyed by the canonical path of the working copy. Two checkouts of the same repository are two projects, because trust attaches to the directory a person chose to open, not to a remote URL.

This composes with the project trust rule already in the requirements. An untrusted project has its proposals ignored entirely, so an unreviewed clone proposes nothing.

## Alternatives considered

User directory only, keyed by project path. Nothing leaks into the repository and nothing is shareable. It is the safe answer and it makes team use worse for no security gain over the split, because the split also enforces from the user store.

Project directory only, committed. Team sharing works with no extra machinery. A pull request that edits the permission file is a privilege escalation that reads like a configuration change, and a fresh clone grants whatever the file says. Rejected on that alone.

Project directory, committed, with a signature. Signing the permission file would stop unauthorized edits. It needs a key distribution story that a coding agent has no business owning.

## Consequences

Two files instead of one, and a prompt flow that handles the difference case. This is more work than either single-location option, and it is the only one that gives sharing without turning a clone into a grant.

The difference prompt needs to be readable. Showing a diff of a permission file is the moment where a user either understands what they are approving or gives up and approves anyway. This deserves interface attention in Phase 1.

Enforcement reads exactly one source. A grant that exists only in the project file never takes effect, which makes the enforcement path simple to audit.

Two requirements follow. WHEN the project proposal set changes after approval, the agent SHALL prompt with the difference before it applies the change. The agent SHALL NOT enforce a grant that exists only in the project file.

## Revisit conditions

Evidence that users approve proposal differences without reading them at a rate that makes the mechanism theater. That would argue for narrowing what a proposal can contain, not for removing the split.
