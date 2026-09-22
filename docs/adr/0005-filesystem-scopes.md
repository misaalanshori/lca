# ADR-0005: Named filesystem scopes

Status: accepted.

Date: 2026-09-20.

## Context

The `fs` capability grants an extension access to part of the filesystem. The first design gave it two scopes: the workspace root and a private per-extension directory.

The question was whether that is enough. The original example was a git extension wanting the git directory, and that example does not hold up, because a git directory sits inside the workspace root and the workspace scope already covers it.

The real gaps are elsewhere. A provider extension may want to read a login that a vendor command line tool already wrote into a global configuration directory. An extension that processes large files wants a temporary directory outside the workspace, so its scratch files do not appear in the user's source tree. A monorepo user may have sibling checkouts that a workflow extension needs to see.

## Decision

Define a fixed vocabulary of named scopes. The manifest names a scope and an access mode. The host resolves the name to a real path. The extension never writes a path into the manifest.

The first vocabulary is `workspace`, `private`, `home-config`, and `temp`. Each entry takes a mode of `read` or `read-write`.

Model the scope name as a string in the manifest, validated against the host vocabulary. A string keeps new scope names additive. A WIT enum would make every new scope an ABI break.

Beyond the vocabulary, the user can attach an extra path grant at install time or later through extension settings. The manifest cannot request one and the consent screen does not offer one. This covers the long tail without putting arbitrary paths in the routine install flow.

Path resolution happens through preopened directory handles. The guest resolves paths relative to a handle. The host never joins a guest-supplied string onto a base path, and it refuses any resolution that leaves the scope through a parent traversal or a symbolic link.

## Alternatives considered

Keep the two fixed scopes. It is the smallest attack surface. It also blocks a provider extension from reading credentials the user already has on disk, which is a normal thing for a provider extension to want, and the workaround would be to ask the user to paste a token that is already sitting in a file.

Let the manifest request arbitrary paths. The most flexible option and the worst consent surface. A manifest asking for the home directory and a manifest asking for the root directory read the same way to most people, which is to say both read as noise. Consent screens that present noise train people to approve without reading, which costs more than the flexibility gains.

A permission prompt on first access rather than a manifest declaration. It defers the decision to the moment it matters, which is good, and it interrupts a running turn with a modal, which is bad. It also removes the ability to review what an extension wants before installing it.

## Consequences

Adding a scope is a host change and a documentation change, not an ABI change. The vocabulary can grow in a patch release.

The consent screen can describe each scope in one sentence that a person can evaluate. This is the property the whole decision exists to protect.

`lca-permissions` owns the vocabulary and the resolution. `lca-ext-host` builds the WASI context preopens from the granted set. Neither the guest nor the ABI knows any real path.

Symbolic link handling needs explicit tests, including a link created after the grant, because that is where this class of bug lives.

Scope resolution and the state-directory exclusion are one function: the host also refuses any resolution that enters the agent's own state directory, sessions, the extension tree, and the credential store, under every scope including an ad hoc grant. This keeps the credential-isolation guarantee in the capability catalog true on platforms, macOS, where the configuration directory conventionally also holds application data.

One requirement follows. IF a guest path resolution leaves its granted scope, THEN the host SHALL refuse the operation and record the attempt.

The ad hoc grant, mentioned above as the pressure valve beyond the fixed vocabulary, later turned out to generalize beyond `fs`. The OpenAI-compatible provider under `docs/providers/openai-compatible.md` needed the same shape of escape hatch for `net`, since its whole reason for existing is a host the manifest cannot know in advance; the alternative, a manifest-declared wildcard, was already rejected on the same "a consent screen has to name something evaluable" grounds this record used for `fs`. The mechanism is the same in both places: something outside the fixed vocabulary, granted at the point of use rather than at install, with consent text naming the specific thing being added rather than a pattern standing in for it. See the capability catalog's `fs` and `net` sections for both.

## Revisit conditions

Three or more real extensions needing a scope the vocabulary does not have, which would argue for a new name rather than a new model. Evidence that users routinely add ad hoc path grants, which would mean the vocabulary is wrong.
