# ADR-0040: Install and update through root-hosted one-liner scripts

Status: accepted.

Date: 2026-09-30.

## Context

The release pipeline publishes six native binaries, `artifacts.sha256`, and a provenance attestation per artifact. What it does not publish is a way to get any of them onto a user's machine that does not involve a human downloading an executable and deciding where to put it. Every prior release asked the user to make that decision themselves, and the decision is where installs quietly fail: a binary in `~/Downloads` that is not executable, a directory that is not on `PATH`, an update that means downloading again and overwriting by hand.

The candidates were a package manager, a custom download domain, a sudo-privileged installer, and a pair of scripts in the repository. Each was rejected or chosen for reasons that are about distribution physics rather than taste.

Package managers (Homebrew, apt, winget, scoop, AUR) are the idiomatic answer on their platforms, and every one of them fails the same three ways here. Each covers one platform, so "supported everywhere" becomes five packaging efforts with five review queues and five staleness stories. None of them can show the extension consent screen — the manifest-as-consent-surface rule that ADR-0005 and the capability catalog are built on — because a package manager's postinstall runs before any of this product's own machinery exists. And they all need an upstream maintainer relationship to stay current, which is a maintenance cost measured in years, not hours. `docs/release-policy.md` already states the conclusion: package manager distribution follows once the release process is stable, and shipping to a package manager before then creates a support burden with stale versions.

A custom domain (`get.lca.dev`) gives a stable URL that never has to be a repository path. It costs a domain renewal forever, a hosting decision, and a DNS-level failure mode that takes the installer down independently of the code, in exchange for moving six characters of URL.

A sudo installer is the shape many tools use, and it is wrong for a product whose whole permission model is "no privilege without an explicit, scoped, revocable grant". Root that persists a binary into `/usr/local/bin` and edits root-owned rc files is a privilege escalation path whose blast radius is the machine, granted once, silently, by a script the user has not read.

That left the repository itself as the host.

## Decision

Four commitments, each of which the others depend on.

**User-local, no sudo.** The sh installer writes to `$HOME/.local/bin` (or `--install-dir`), the Windows installer to `$env:LOCALAPPDATA\lca\bin`. Nothing needs elevation; an install that needs sudo fails on a locked-down machine and succeeds only where the user has already decided to hand over root.

**The scripts live at the repository root on `main`, and the raw URL is the product.** `https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh` serves the repository's own bytes for that path — no build step, no deploy step, no CDN configuration, no second source of truth. The script a user executes is the script a reviewer reads at the same URL. The cost is that the script's history is the repository's history, which is exactly what a security-sensitive executable wants.

**Version resolution through `releases/latest/download`, not the API.** "What is the newest release" is one HTTP request to a redirect GitHub already serves for any asset on the latest release: no API call, no JSON parsing, no token, no rate limit to hit and no quota to explain. `--version` swaps the pattern for `releases/download/v<V>/`. The consequence worth stating: a *moving* pointer is resolved at install time and pinned into the installed artifact, so the URL is mutable and the installed file is not — the same split the extension lockfile makes between a moving tag and a digest (ADR-0009).

**Verification is mandatory and the same script is the updater.** `artifacts.sha256` is fetched from the same directory as the binary and checked before the file is moved into place; a mismatch exits non-zero with the binary untouched. Re-running the one-liner is the update path, because an updater nobody remembers is not an updater: the script prints `lca <old> -> <new>` from the version line it read before replacing the file. `--uninstall` is the same script again, removing the binary and only the marked block it added.

The PATH edit is an idempotent marked block in the user's rc file, never a rewrite of the file and never a PATH that lives only in the installing shell's environment. On Windows the same rule is the user `Path` registry value, appended without clobbering and with the original value kind preserved.

## Alternatives considered

A package manager per platform, above. It is the right long-term channel and it is not the first channel.

A custom domain for a stable URL. Money, DNS, and an availability story independent of the repository, for six characters of savings.

A sudo installer writing to `/usr/local/bin`. Policy: the product does not acquire privilege it does not need, and an installer is the worst possible place to be generous with sudo.

A GitHub API call for the latest release (`/releases/latest`). It returns JSON that has to be parsed by a `sh` script with no guaranteed `jq`, it is rate-limited per IP, and it authenticates badly from a CI runner. The redirect already answers the question.

A self-updating binary (download in place, exec over itself) or a background updater daemon. Both add a process the user did not start to a product that deliberately has no daemon (SRDD process model), and both make the update itself a mutation of a running file. The re-run-the-one-liner model keeps every update in front of the user, in the same shell, with the same verification.

A separate `install` crate or binary in the workspace. It would need building and publishing before it could install the thing it is part of.

Bundling a checksum skip for users who complain. Rejected without discussion: verification that can be turned off by a flag will be turned off by a flag.

## Consequences

The repository root gains two files that are executed by strangers by default, which raises their review bar to "shellcheck-clean POSIX sh with no hidden fetch and no `eval`" — `scripts/install-check.sh` (gate 10) and the `install` CI job enforce that on every push.

`raw.githubusercontent.com` is now a load-bearing dependency of the install path. If it is unreachable, installs fail; the scripts themselves are also fetchable from the release page or the repository, and `docs/installation.md` documents the manual path as a first-class option rather than a footnote.

Because the scripts are the update path, a defect in `install.sh` ships to every user at the same instant it lands on `main`, with no release gate between. The mitigation is that the script does one thing (fetch, verify, move) and every branch of it is covered by `tests/install/test_install_sh.sh` before it lands.

The macOS quarantine strip is documented as a deliberate act with a stated trust chain (checksum, then attestation) instead of being left as an unexplained `xattr` line in a script people pipe into `sh`.

`docs/release-policy.md`'s gate list gains gate 10, `scripts/install-check.sh`, and the requirement that both one-liners stay green in CI is what keeps "the URL always works" from being a claim rather than a checked fact.

## Revisit conditions

Evidence that a real, motivated third-party packager exists for any platform (a Homebrew cask maintainer, a winget manifest upstreamed by someone other than us) — at that point the installer stays as the direct channel and the package becomes a second one, per the release policy's ordering.

Evidence that raw.githubusercontent rate-limiting or availability actually blocks installs in the wild; the fallback then is publishing the scripts as release assets and pointing the one-liner at a pinned release URL, which costs the "always latest script" property and gains immutability.

Evidence that users routinely pipe the script into a shell without reading it *and* ask for a stronger first-run story, which would justify signing the script itself rather than strengthening the documentation around reading it first.
