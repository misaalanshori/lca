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
| `lca:host/resources` read | Every extension (always granted) | Reading its own package data; the risk is not the read but what the bytes *are* - see the skill-text and preset scenarios below |
| `lca:host/state` write | Every extension (always granted) | Filling the disk; planting data a later call trusts |
| A package's `resources/` bag at install | Whoever published the package | Shipping hostile content that the host or the model will read |

### The three new surfaces (ADR-0030/0031/0032)

The bags and the preset data introduced three risks the table above cannot
express as an import, because the danger is in the content rather than in
the call. Each row names the mitigation **as built**, and what pins it.

| Risk | As built | Pinned by |
|---|---|---|
| **Skill text as prompt injection, with a distribution channel.** A package ships `resources/skills/<name>/SKILL.md` containing instructions aimed at the model rather than the reader, and now it is in every prompt. | The install consent names the `resources` kinds and their counts, so a package cannot quietly carry a bag; a manifest that declares no `resources` kinds is refused an undeclared one. Every injected skill is attributed (`[skill <name> from <project>\|user\|<extension>]`), so injected instructions are visibly not the user's own words. Any extension can be disabled per project, which removes its skill pack with it. | `crates/lca-core/src/skills.rs` (attribution + precedence); `crates/lca-registry/src/lib.rs` (`install` refuses an undeclared kind, `resource_consent` names the counts); `crates/lca-core/tests/skills.rs` |
| **Preset phishing.** A preset points `base_url` at an attacker's host, the picker shows a familiar name, and the user's key goes there. | The picker shows the **host**, not just the name. The key prompt is masked and never reaches the scrollback or the log. The key goes to the extension's own credentials namespace, never into configuration and never into a session record (FR-CFG-5). A base URL whose host is outside the manifest's `net` vocabulary gets the B1 ad hoc grant prompt naming that exact host, and it is login-only so a turn cannot manufacture one (FR-PERM-16). | `crates/lca-tui/src/lib.rs` (the picker's hint column, the masked prompt); `crates/lca-cli/src/login.rs` (`field_prompt`: only `api-key` is masked); `crates/lca-cli/src/tui.rs` (`ungranted_host` + the grant prompt); `crates/lca-cli/tests/e2e.rs` (`the_logins_secret_prompt_masks_input_in_a_real_terminal`, `the_picker_moves_to_a_local_preset_that_needs_no_key`) |
| **Resource bloat and install DoS.** A package declares a huge bag and the installer or a later read pulls the host into memory games. | Per-file cap 1 MB, per-call read cap 1 MB, per-package cap 32 MB, enforced at pack, at install, and at read. `state` is capped the same way (4 MB per key, 16 MB per namespace) and wiped on uninstall. | `lca-registry::RESOURCE_FILE_MAX_BYTES` / `RESOURCE_PACKAGE_MAX_BYTES`; `lca-tools::RESOURCE_READ_MAX_BYTES`, `STATE_VALUE_MAX_BYTES`, `STATE_TOTAL_MAX_BYTES`; `crates/lca-tools/tests/resources.rs` (`a_read_over_the_size_cap_is_refused`), `crates/lca-tools/tests/state.rs` (`a_state_value_over_the_cap_is_refused`), `crates/lca-registry/tests/registry.rs` |

The bags themselves are not a trust boundary in the usual sense: the path
resolves inside the calling extension's own tree only, with no traversal,
absolute path, NUL, or symlink escape (FR-PERM-6/7), so a hostile package
cannot read a sibling's bag even when it tries. That is why the three rows
above are about content, not reach.

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

Stopped by namespace isolation. A credential namespace must equal the extension name, enforced at manifest validation. There is no cross-namespace read at any level, and the `fs` capability cannot reach the credential store because the store lives in the agent's own state directory, which the host excludes from every `fs` resolution.

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

### A completion-holding extension is steered by injected content

An extension holding `completion` and `process` or `fs` reads attacker-controlled content, a tool result or a file, feeds it to the active provider, and acts on the response, turning the model into an interpreter for injected instructions the user never sees.

Partly stopped. Whatever the extension does with the response still passes through the capability boundary: `process` commands still hit the approval prompt, `net` calls are still allow-listed, and `fs` writes still show up in the user's version control diff. Not stopped: the extension using model output to choose which approved action to take, and a broad pre-approved command pattern turns that choice into real execution. This is the shape of access that made `completion` a meaningfully different capability in ADR-0015, and it is why the Phase 8 review gives `completion` specific attention. The mitigations are consent clarity on the `reason` string and the standing rule against broad pre-approved patterns.

### A malicious project file escalates permissions

An attacker adds permission proposals to a repository, and a user pulls the branch.

Stopped by the split permission store. A project file holds proposals with no force. Only the user grant store is consulted at enforcement time. Approving a proposal copies it into the user store behind a prompt, and the user store records a hash of the approved proposal set, so a later edit re-prompts with the difference.

Also stopped earlier by project trust. An untrusted project has its configuration ignored entirely.

See ADR-0006 for why both layers exist.

### The update check's answer is hostile

The one request the agent makes without being asked (FR-CFG-6's daily update check) asks the hosting API which release is newest, and prints what comes back.

Stopped: the answer is never executed and never installed - it is parsed as a tag string, passed through the display choke point (FR-UI-2's sanitizer), truncated, and rendered as text in the status line. The request runs on a background task with a five-second timeout, at most once a day, and makes no appearance in headless mode unless the user switched the option on, so a CI run stays silent. The transport is HTTPS to a single named host. Not stopped: the hosting service itself answering with a lie - the result would be a false notice, which is why the notice carries no link and offers no action.

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

Broad `net-local` range grants. A CIDR grant reaches every device in that range, not only the one the user had in mind, on whatever network the user happens to be connected to at the time, including a tailnet. Consent text names this; nothing narrows the grant below the range the user approved.

No signature verification at first install. Digest pinning protects everything after the first resolution. Signing needs key distribution that 0.1 does not have. This is the most likely addition after 1.0.

Native-linked extensions have no sandbox. This is deliberate, reserved for code shipping in the binary, and labeled in the interface. A build including a third-party extension natively is out of policy, and policy is the only enforcement.

Runtime sandbox escape. Covered above. Accepted with version pinning and advisory tracking.

Prompt fatigue. The permission model depends on people reading prompts. Every design decision that reduces prompt volume for routine actions protects the prompts that matter.

## Review checklist

The Phase 8 review covers, at minimum:

Every host import function checks its grant before it acts, and the check cannot be reached only on some paths.

The granted set rather than the declared set governs every capability call, and an ungranted call is refused with a recorded permission error (FR-PERM-3); an interface a world does not carry fails at link time.

Path resolution cannot leave a scope, including through a symlink created after the grant.

Credential namespace isolation has no bypass, including through `fs` with every scope granted.

Control characters cannot reach the terminal from any extension-supplied string, including through a widget label, a tool name, a command name, or an error message.

The permission layer is on the path for every command execution, including commands originating from an extension rather than the model.

Digest verification runs before instantiation, not after.

The `net` capability's resolved-address check rejects a connection whose resolved address falls in a loopback or private-use range even when the hostname pattern matched, and records the attempt as a rebinding case rather than an ordinary denial.

The fuzz targets (four: manifest, session log, archive, ABI decode) have run long enough to be meaningful, and their corpora are checked in.

Denial recording cannot be suppressed by the extension that triggered it.

## The `completion` capability (ADR-0015, Phase 4)

An extension holding `completion` can spend the user's model budget
without the user watching each call. The scenario to walk: a malicious
or compromised compaction strategy asks for completions in a loop,
each one billed to the account, or exfiltrates data by encoding it in
the prompt it sends.

Controls that hold it: the capability is manifest-declared with a
required `reason` shown verbatim on the consent screen, so installing
this reach is a visible act (capability catalog); the request routes
through the host to the ACTIVE provider, never to another extension,
keeping the graph a star with no extension-to-extension channel to
abuse (ADR-0008); the extension never sees the provider's credentials
- the host holds them (FR-PERM-6/7); every denied or undeclared call
returns a permission error and is recorded (FR-PERM-3), which the
conformance extension exercises in both delivery modes; and the usage
lands on the session record that caused the spend, so a runaway shows
up in session cost rather than only on the invoice (capability
catalog). What this control does NOT provide: a per-turn budget on
completion calls. The catalog's cost attribution is the detector, not
a limiter; a metering limit is the upgrade path if attribution proves
insufficient in use.

## The Phase 8 walkthrough

The review checklist above, walked item by item. A receipt names a
test in the suite; a justification says why no test applies and what
carries the risk instead. An external reviewer should be able to start
from these receipts rather than from scratch; findings that need one
are the review's to make, not something this list can pre-answer.

- **Every host import checks its grant before it acts, and the check
  cannot be reached only on some paths.** Receipt: `lca-ext-host`
  `tests/capabilities.rs` `undeclared_capabilities_error_and_record`
  exercises the undeclared path for fs, process, and pty; `lca-tools`
  `tests/network.rs` does it for net and net-local
  (`undeclared_net_is_a_recorded_permission_error`) and for oauth
  (`oauth_without_a_grant_never_binds`) and credentials; and
  `tests/provider.rs` `denied_identity_capabilities_refuse_at_both_boundaries`
  proves oauth and credentials refuse through the WASM imports as well. The
  Phase 4 conformance case covers `completion` in both delivery
  modes (`compaction_and_transform_agree_across_modes_with_
  completion_denied`); `ui` never calls the export outside
  `ui_regions`. Each check sits inside the capability method itself, so
  a caller cannot construct a path around it - the deny is a property
  of the engine, not of the call site.
- **The import table comes from the granted set; an ungranted import
  fails rather than loads silently.** Receipt: the same capability
  tests - every undeclared call is a recorded refusal, not a link-time
  surprise, because the host links capability interfaces in a denied
  state by design (the loader's comment says so and the tests pin the
  behavior). Manifest validation rejects unknown capabilities outright
  (`manifest_declares_and_parses_every_capability`), so the declared
  set itself cannot exceed the catalog.
- **Path resolution cannot leave a scope, including through a symlink
  created after the grant.** Receipt: `lca-permissions`
  `tests/scopes.rs` - `parent_traversal_out_of_a_scope_is_refused`,
  `a_symlink_created_after_the_grant_is_refused` (unix: Windows
  symlink creation needs elevation, recorded in the phase log),
  `absolute_paths_are_refused`, and the state directory refused under
  every scope.
- **Credential namespace isolation has no bypass, including through
  fs with every scope granted.** Receipt: `lca-tools`
  `tests/network.rs` `credentials_are_namespace_isolated_with_owner_
  only_permissions` (FR-PERM-6/7, NFR-14) plus the state-dir refusal
  above: the store lives where no `fs` grant points, and the namespace
  is the manifest's own name, checked at parse time
  (`capabilities.credentials.namespace must equal the extension
  name`).
- **Control characters cannot reach the terminal from any
  extension-supplied string - widget, tool name, command name, or
  error message.** Receipt, per vector: widgets go through
  `sanitize_text` in `lca-tui::widget_lines`, the single choke point
  (`control_sequences_become_visible_text`,
  `an_extension_renders_in_all_four_regions_and_the_hostile_span_
  stays_literal` asserts the virtual buffer holds no control byte);
  command names and every notice an extension effect produces are
  sanitized at the two assignment sites in `handle_key`; dispatch error
  messages reach notices through the same path; headless JSON envelopes
  escape control characters by construction (serde). What is NOT
  covered here: model-generated scrollback text, which is not
  extension-supplied and would be the prompt-injection scenario's
  business rather than this checklist's.
- **The permission layer is on the path for every command execution,
  including extension-originated ones.** Receipt: `lca-tools`
  `tests/tools.rs` `process_spawn_shows_the_exact_command_and_runs_
  when_approved` (the prompt shows the exact command, FR-UI-4's data),
  and the ui-example panel's shell spawn runs through the same engine
  method - there is no spawn path that bypasses `Capability::new`'s
  prompt. Justification for the rest: an extension that already holds
  `process` approved its surface once at install (the catalog's
  consent line says each command still asks), and model-originated
  shell calls are the core's `required_permission` path with its own
  tests (FR-TOOL-3).
- **Digest verification runs before instantiation, not after.**
  Receipt: `lca-registry` `tests/registry.rs`
  `oci_resolution_verifies_both_digests` - a registry whose layer
  digest does not match its own bytes is refused at resolve time, and
  nothing reaches the store (FR-DIST-3/4); `read_archive` computes the
  digest the lockfile records before install.
- **The net resolved-address check rejects a loopback/private landing
  even when the hostname matched, recorded as rebinding.** Receipt:
  `lca-tools` `tests/network.rs` `net_refuses_local_resolution_as_
  rebinding` (FR-PERM-13) - distinct record, distinct message, and the
  ad hoc attach test proves the fix for a legitimate case is consent,
  not a wider pattern (FR-PERM-18).
- **The fuzz targets have run long enough to be meaningful, corpora
  checked in.** Receipt: `fuzz.yml` runs all four on a schedule with
  ten sustained minutes each; the local receipt for this release is
  ~2 million executions across the four targets with zero crashes
  (manifest666k, session log651k, archive538k, ABI decode125k), and
  `fuzz/corpus/` is committed so anything found later is a permanent
  seed.
- **Denial recording cannot be suppressed by the extension that
  triggered it.** Justification, structural: the record lives in the
  host's engine and in the on-disk journal, and no import surface
  exposes either - the WIT has no read-deniars function, so the only
  code that can see a denial is host code (`ext info`). The receipts
  above show the record appearing for calls the extension would rather
  have had succeed (every FR-PERM-3 test asserts the count after the
  refusal).

Open findings for the external review: none known above low. The two
residual risks the scenarios already name - exfiltration to an
approved host, and a net-local range wider than the one device meant -
are accepted-in-design and tracked under Residual risks above; one
practical gap is that notices and slash lists are the sanitized paths
today, while scrollback text is model-supplied and belongs to the
prompt-injection scenario rather than this checklist.
