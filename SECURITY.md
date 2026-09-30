# Security

## Reporting a vulnerability

Use GitHub's private reporting for this repository:
**Report a vulnerability** under
<https://github.com/misaalanshori/lca/security/advisories>. If that route is
unavailable, open an issue asking for a contact address and put no exploit
detail in it.

You get an acknowledgment within a few days and a coordinated disclosure
date. A fix ships as a patch release on the current minor version, and on the
previous minor version while it is inside the support window, with the
advisory published alongside it. The advisory names the affected versions,
the impact, and the fix version; it does not carry a working exploit. The
full policy is [`docs/release-policy.md`](docs/release-policy.md).

## What matters most

Anything in these boundaries is treated at the highest urgency, because the
whole design rests on them:

- the capability enforcement path (`crates/lca-ext-host`, `crates/lca-tools`
  capability checks) — a grant that is not enforced is a broken product,
- the permission layer and the grant store (`crates/lca-permissions`),
- the credential store and per-extension credential namespacing,
- the installers (`install.sh`, `install.ps1`) — a checksum that can be
  skipped or a PATH write that happens before verification is in scope.

A WebAssembly sandbox escape is a host compromise by design; the mitigations
and their limits are written down in `docs/threat-model.md`, and a finding
that defeats the sandbox is in scope even though the document says so.

## Not in scope

Denial of service against yourself (a slow model, a huge file), issues in
dependencies with no path through LCA's own surfaces (report them upstream),
or anything that requires an extension the user knowingly installed and
granted full capabilities to already.
