# Threat model

Version 0.1, 2026-09-20.

This document enumerates what an attacker can reach, what the design stops, and what it does not. It is written to be reviewed against. The Phase 8 security review uses it as the checklist, so a mitigation claimed here needs a test somewhere.

## Assets

What an attacker wants, roughly in order of value.

Credentials in the credential store: API keys, access tokens, and refresh tokens for model providers and any service an extension talks to.

The user's source code and any file the agent can read. This includes files outside the workspace when a grant or a prompt allowed it.

Command execution on the user's machine, which is the most valuable and the most direct path to everything else.

Session history, which holds conversation content, file excerpts, and command output.

The user's model spend, which an attacker can burn through without ever getting data out.

## Trust boundaries

Five boundaries matter. Everything in this document happens at one of them.

The user and the agent. The user is trusted. Anything the user explicitly approves is authorized by definition, which makes the clarity of a consent prompt a security property rather than a usability one.

The model and the agent. The model is untrusted. It sees file content, command output, and web content, any of which can carry injected instructions. Everything the model asks for passes through the permission layer.

An extension and the host. An extension is untrusted. The WASM sandbox and the capability import table are the boundary.

The registry and the agent. A registry serves bytes. Digest verification is the boundary.

The project directory and the agent. A repository is untrusted until the user marks it trusted. Project configuration and permission proposals sit on the far side of this boundary.

## Actors

A malicious or compromised extension. It was installed, possibly legitimately, and now contains hostile code. Either the author turned, the author's account was taken, or a dependency in the author's build was poisoned.

An attacker writing content the model will read. A file in a repository, a web page, a dependency README, a commit message, an issue body. This is prompt injection and it needs no access to the user's machine.

An attacker with write access to a repository the user works in. They can edit project configuration, permission proposals, and any file the agent might read.

An attacker who controls or compromises a registry, or who can intercept a fetch.

An attacker with local access to the user's machine, outside the agent entirely. Mostly out of scope, noted where a design choice affects it.

## Attack surface

The host import table is the enumerable surface. Every function in it is an entry point an extension can call.

| Entry point | Reachable by | Primary risk |
|---|---|---|
| `lca:host/net` request | Extension with `net` | Data exfiltration to an attacker-controlled host; DNS rebinding to a local address |
| `lca:host/net` request (local) | Extension with `net-local` | Reaching another device on the user's network, not just the user's own machine |
| `lca:host/fs` read, write | Extension with `fs` | Reading secrets, writing a backdoor into source |
| `lca:host/credentials` get, set | Extension with `credentials` | Token theft |
| `lca:host/oauth` begin, await | Extension with `oauth` | Phishing a sign-in, capturing a code |
| `lca:host/process` spawn | Extension with `process` | Arbitrary code execution |
| `lca:host/pty` spawn | Extension with `pty` | Arbitrary code execution with a real terminal, same class as `process` |
| `lca:host/completion` request | Extension with `completion` | Consuming model output as input to extension logic; a channel for the extension to react to content it could not otherwise see |
| `lca:host/ui` render | Extension with `ui` | Spoofing a prompt, hiding output |
| `lca:host/log` | Every extension | Noise, minor information disclosure in diagnostics |

Outside the import table: the manifest parser, the session log reader, the OCI client, the HTTPS archive resolver, and the canonical ABI decode path all read input an attacker can influence. These are the fuzz targets.

## Scenarios

### A malicious extension exfiltrates credentials

The extension reads its own tokens through `credentials` and tries to send them somewhere.

Stopped by the `net` allow list. The host checks the target host against the patterns the user approved. A request to an unlisted host is refused and recorded. The extension cannot open a socket, so there is no path around the check.

Not stopped: exfiltration to a host the user already approved. A provider extension for a real service can send anything to that service's own API, and no allow list can distinguish a completion request from a payload. This is a residual risk, noted below.

### DNS rebinding bypasses the net allow list

An extension declares a `net` grant for an ordinary-looking public hostname. At connection time, that hostname resolves to a loopback or private-network address, either through misconfiguration or a deliberate attacker-controlled DNS response, reaching a target the pattern match never intended to expose.

Stopped by resolving the hostname host-side and checking the resolved address, not just the matched pattern, against the loopback and private-use ranges before connecting. A resolution landing in those ranges is refused and recorded as a rebinding attempt specifically, distinct from an ordinary denial. See ADR-0011.

### A local-network extension reaches an unintended device

An extension holding `net-local` is granted a private-use CIDR range broader than the single device the user had in mind, for example the whole `192.168.0.0/16` block to reach one local model server, and the user is on a network, such as a shared office or public wifi with local routing enabled, where other devices sit in that same range.

Not fully stopped. `net-local`'s consent text says plainly that the grant reaches other devices on the network, not just the user's own machine, but a range grant is inherently coarser than the one device the user actually intended. This is a residual risk, noted below, and the design accepts it in the same spirit that `process` accepts the risk of an overly broad pre-approved shell pattern: consent clarity is the primary defense, not enforcement narrower than what the user actually asked to grant.

### One extension reads another's tokens

Stopped by namespace isolation. A credential namespace must equal the extension name, enforced at manifest validation. There is no cross-namespace read at any level, and the `fs` capability cannot reach the credential store because the store lives outside every scope.

### A malicious extension writes a backdoor into source

An extension with `fs` write access to the workspace modifies a build script or a source file.

Partly stopped. The write is allowed if the user granted workspace write, which many legitimate extensions need. What stops the attack in practice is that the change appears in the user's version control diff.

The mitigation is consent clarity, not enforcement: an extension asking for `read-write` on the workspace should have to justify it, and the consent screen distinguishes read from write.

### A malicious extension runs a command

Not possible without the `process` capability, and holding `process` only means the extension may ask. Every command still passes through the user approval prompt, and a pre-approved pattern in the grant store satisfies it the same way it does for a model-requested command.

The real risk here is a broad pre-approved pattern. A user who approved `git *` has approved `git config core.pager` pointing at an arbitrary program. Pattern approval is where this model is weakest, and the interface should discourage broad patterns.

### A malicious extension spoofs a permission prompt

Stopped by the rendering model. An extension returns a widget tree, not terminal bytes. It cannot draw outside its granted region, cannot move the cursor, and cannot emit control codes. Text spans are stripped of control characters before drawing.

This is the whole reason the widget tree exists. See ADR-0003.

### Prompt injection causes a destructive command

A file the model reads contains text instructing it to run a destructive command. The model complies.

Stopped by the permission layer. The model cannot run anything directly. The command reaches the user as a prompt showing the exact command.

Weakened by pre-approved patterns and by prompt fatigue. A user who approves quickly is the failure mode, which is why prompts show the exact command rather than a summary, and why broad patterns deserve interface friction.

Hooks give a second layer. A policy extension can deny calls by pattern before the prompt appears, which is the recommended shape for a team that wants a hard rule rather than a per-user decision.

### Prompt injection exfiltrates data through a tool

Injected text tells the model to read a secret and include it in a request to an attacker's host, perhaps through a web fetch tool or a command with a URL.

Partly stopped. A built-in tool that makes network requests is subject to the permission layer, and a command with a URL shows the URL in the prompt. An extension-provided fetch tool is subject to the extension's own `net` allow list.

Not stopped: an injected instruction to write a secret into a file the attacker can later read, in a repository the attacker has access to. Version control review is the defense.

### A malicious project file escalates permissions

An attacker adds permission proposals to a repository, and a user pulls the branch.

Stopped by the split permission store. A project file holds proposals with no force. Only the user grant store is consulted at enforcement time. Approving a proposal copies it into the user store behind a prompt, and the user store records a hash of the approved proposal set, so a later edit re-prompts with the difference.

Also stopped earlier by project trust. An untrusted project has its configuration ignored entirely.

See ADR-0006 for why both layers exist.

### A compromised registry serves a different component

Stopped for an installed extension by digest pinning. The lockfile records a digest and the host loads by digest, verifying before instantiation. A mismatch refuses the load and reports tampering.

Not stopped at first install. The first resolution of a tag to a digest trusts the registry and the transport. Transport is HTTPS with host certificate verification. There is no signature check in 0.1, which is a residual risk.

An update resolves the ABI line tag to a new digest, which is a trust decision at that moment. An update that widens the capability set prompts before applying.

### A malicious session file

A crafted session log or a manifest with pathological content triggers a parser bug.

Mitigated by fuzzing. The manifest parser, the session log reader, and the canonical ABI decode path are the three fuzz targets, chosen because they are the three places untrusted bytes are parsed.

The session reader also fails safely by design: it stops at the first unparseable record, keeps what came before, and reports truncation.

### A sandbox escape in the runtime

An extension exploits a defect in Wasmtime to run code in the host process.

Not stopped by anything in LCA. This is a full host compromise.

Mitigations are indirect: pin the runtime version, track advisories with the same urgency as a defect in LCA's own enforcement code, keep the host import surface small so there is less to get wrong, and prefer the interpreter backend on targets where executable memory is a concern.

### Path traversal out of a granted scope

An extension supplies a path with parent traversal, or follows a symbolic link pointing outside its scope.

Stopped by resolution through preopened directory handles. The host never joins a guest-supplied string onto a base path. A resolution that leaves the scope is refused and recorded separately from an ordinary denial, because it indicates a defect or an attack rather than a missing grant.

Symbolic links created after the grant need explicit tests. That is where this class of bug lives.

## Residual risks

These are accepted, not solved. Each needs a decision if it becomes real.

Exfiltration to an approved host. A provider extension can send arbitrary data to the service it legitimately talks to. No allow list distinguishes intent. Mitigation would need content inspection, which is out of scope and probably ineffective.

Broad pre-approved command patterns. The grant store lets a user approve a pattern wide enough to cover anything. Interface friction is the only current defense.

Broad `net-local` range grants. A CIDR grant reaches every device in that range, not only the one the user had in mind, on whatever network the user happens to be connected to at the time. Consent text names this; nothing narrows the grant below the range the user approved.

No signature verification at first install. Digest pinning protects everything after the first resolution. Signing needs key distribution that 0.1 does not have. This is the most likely addition after 1.0.

Native-linked extensions have no sandbox. This is deliberate, reserved for code shipping in the binary, and labeled in the interface. A build including a third-party extension natively is out of policy, and policy is the only enforcement.

Runtime sandbox escape. Covered above. Accepted with version pinning and advisory tracking.

Prompt fatigue. The permission model depends on people reading prompts. Every design decision that reduces prompt volume for routine actions protects the prompts that matter.

## Review checklist

The Phase 8 review covers, at minimum:

Every host import function checks its grant before it acts, and the check cannot be reached only on some paths.

The import table is built from the granted set rather than the declared set, and a component needing an ungranted import fails at link time.

Path resolution cannot leave a scope, including through a symlink created after the grant.

Credential namespace isolation has no bypass, including through `fs` with every scope granted.

Control characters cannot reach the terminal from any extension-supplied string, including through a widget label, a tool name, a command name, or an error message.

The permission layer is on the path for every command execution, including commands originating from an extension rather than the model.

Digest verification runs before instantiation, not after.

The `net` capability's resolved-address check rejects a connection whose resolved address falls in a loopback or private-use range even when the hostname pattern matched, and records the attempt as a rebinding case rather than an ordinary denial.

The three fuzz targets have run long enough to be meaningful, and their corpora are checked in.

Denial recording cannot be suppressed by the extension that triggered it.
