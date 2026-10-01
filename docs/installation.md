# Installation: the one-liner installers, and installing by hand

Version 0.1, 2026-09-30.

Two scripts at the repository root, `install.sh` (POSIX sh: Linux and macOS) and `install.ps1` (Windows PowerShell), are the primary distribution channel named in `docs/release-policy.md`. Both are hosted at the repository root on `main` and served through `raw.githubusercontent.com`, so the documented URL always resolves to the current script, and the same script is the updater: run the one-liner again and the install is replaced in place.

This document is the specification both scripts implement. The decision record is `docs/adr/0040-install-and-update.md`; the requirements are `FR-INSTALL-1` through `FR-INSTALL-9` in `docs/lca-srdd.md`; the tests are `tests/install/`, gated by `scripts/install-check.sh` (gate 10 in `docs/release-policy.md`'s gate list).

## The one-liners

Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh | sh
```

Windows (Windows PowerShell 5.1 and pwsh):

```powershell
irm https://raw.githubusercontent.com/misaalanshori/lca/main/install.ps1 | iex
```

A pinned version passes as an argument. The sh form is an ordinary invocation of the downloaded script; the PowerShell form wraps the fetched text in a script block so named parameters bind on 5.1 (verified in CI, not on paper):

```sh
curl -fsSL https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh | sh -s -- --version v0.5.2
```

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/misaalanshori/lca/main/install.ps1))) -Version v0.5.2
```

## Flags

### `install.sh`

| Flag | Meaning |
|---|---|
| `--version <X.Y.Z\|vX.Y.Z>` | Install that release instead of the latest. |
| `--unstable` (`-Unstable`) | Install the **unstable line** instead: the rolling pre-release at `<base>/download/unstable/<A>`, newest green commit, `X.Y.Z.b<sha7>`. Latest-only (ADR-0043), so combining it with `--version` is a usage error (exit 2). |
| `--install-dir <dir>` | Install directory. Default `$HOME/.local/bin`; the `LCA_INSTALL_DIR` environment variable supplies the default when set, and the flag wins over both (flag > environment > default). |
| `--no-path` | Do not edit any shell rc file. |
| `--uninstall` | Remove the installed binary and every marked PATH block this installer added, then report what was removed. |
| `--help` | Print usage, including the one-liners themselves. |

### `install.ps1`

| Parameter | Meaning |
|---|---|
| `-Version <X.Y.Z\|vX.Y.Z>` | Install that release instead of the latest. |
| `-InstallDir <dir>` | Install directory. Default `$env:LOCALAPPDATA\lca\bin`; `LCA_INSTALL_DIR` supplies the default when set, and the parameter wins over both. |
| `-NoPath` | Do not edit the user `Path`. |
| `-Uninstall` | Remove the installed binary and the PATH entry, then report what was removed. |
| `-BaseUrl <url>` | Mirror/test seam (same meaning as `LCA_BASE_URL`). |
| `-Verbose` | Print the download URL before fetching. |

`LCA_BASE_URL` (sh) and `-BaseUrl` (ps1) replace the release URL prefix (FR-INSTALL-6). The environment variable works on both scripts.

## The unstable line

`--unstable` / `-Unstable` swaps one thing: the URL pattern above goes to `download/unstable/`, where a rolling pre-release carries the newest commit that passed the whole pipeline (ADR-0043). Everything else — platform mapping, the mandatory checksum from the same directory, PATH handling, `--no-path`, `--uninstall` — is the same code path as a stable install.

```sh
curl -fsSL https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh | sh -s -- --unstable
```

Switching lines is just running the other form: no flag installs (or re-installs) the stable line over whatever is there, the flag installs the unstable line, and the report line says which is which (`lca 0.5.2.b6573049 -> 0.5.2`). The installed binary keeps whatever it was given; nothing about a line is stored, so the next run decides again.

**Trust stance.** The unstable line verifies exactly like the stable one: `artifacts.sha256` from the same directory, mismatch refuses and leaves the old binary in place, and the same `actions/attest-build-provenance` attestation is produced per artifact, so `gh attestation verify ./lca --repo misaalanshori/lca` works on an unstable binary too. What it does not carry is a promise: no changelog entry, no support window, and a build that may be broken by design. When in doubt, run the one-liner without the flag.

## Platform matrix

The six release assets are the six names `docs/release-policy.md` publishes; the mapping is exactly this (FR-INSTALL-7 covers everything outside the table):

| Script | Detected | Asset |
|---|---|---|
| sh | `uname -s` = `Linux`, `uname -m` = `x86_64` | `lca-x86_64-unknown-linux-musl` |
| sh | `uname -s` = `Linux`, `uname -m` = `aarch64` or `arm64` | `lca-aarch64-unknown-linux-musl` |
| sh | `uname -s` = `Darwin`, `uname -m` = `x86_64` | `lca-x86_64-apple-darwin` |
| sh | `uname -s` = `Darwin`, `uname -m` = `arm64` or `aarch64` | `lca-aarch64-apple-darwin` |
| ps1 | `$env:PROCESSOR_ARCHITECTURE` = `AMD64` | `lca-x86_64-pc-windows-msvc.exe` |
| ps1 | `$env:PROCESSOR_ARCHITECTURE` = `ARM64` | `lca-aarch64-pc-windows-msvc.exe` |

A Linux `uname -s`/`uname -m` pair outside the table exits **2** with a message that names the pair and points a Windows user at the `install.ps1` one-liner. A `$env:PROCESSOR_ARCHITECTURE` outside `AMD64`/`ARM64` exits **1** with the same naming discipline. The sh script never attempts a Windows install: `uname` cannot report one.

## URLs

One base, two patterns, for asset `A` and optional version `V`:

- default base: `https://github.com/misaalanshori/lca/releases`
- latest: `<base>/latest/download/<A>`
- pinned: `<base>/download/v<V>/<A>`
- unstable: `<base>/download/unstable/<A>` (`--unstable`/`-Unstable`)

`releases/latest/download` is a redirect GitHub resolves for any asset on the latest release, so "what is the newest version" costs one HTTP request with no API call, no JSON parsing, and no rate limit (ADR-0040). The checksum file sits beside the binary under the name `artifacts.sha256`, fetched with the same pattern — the unstable line's checksums live beside its binaries in `download/unstable/`, so verification is identical on both lines (FR-INSTALL-10).

`LCA_BASE_URL` (or `-BaseUrl`) replaces the base and nothing else, so a mirror with the same layout is a drop-in (FR-INSTALL-6). A base that is a local path — `file://` or a bare directory — is read with a copy instead of a network fetch, which is what makes the test suite hermetic; the fixture directory mirrors the release layout (`<base>/latest/download/`, `<base>/download/v<V>/`).

## What the installer does, in order

1. Resolve platform → asset; resolve version → URL prefix.
2. Create a temporary directory (`mktemp -d`); download `artifacts.sha256` and the asset into it.
3. **Verify**: find the asset's line in `artifacts.sha256` and compare hashes — `sha256sum` on Linux, `shasum -a 256` on macOS, `Get-FileHash -Algorithm SHA256` on Windows. No checksum tool found is a hard failure, not a skip. Mismatch: delete the download, print the expected and actual digest, exit **1** (FR-INSTALL-2).
4. `chmod 755` the asset in the temp directory (Windows: nothing to chmod).
5. **Atomic install**: `mv` the verified file onto `<install-dir>/lca` (`lca.exe` on Windows), falling back to copy+remove across filesystems. Because the move is last, a failure anywhere above leaves an existing binary untouched (FR-INSTALL-1).
6. macOS only, best effort: `xattr -d com.apple.quarantine <bin> || true` (see "Security stance" below).
7. PATH management unless `--no-path`/`-NoPath` (FR-INSTALL-3).
8. Print what was installed, where, and the source rc file to `source` — or, on an update, `lca <old> -> <new>` (FR-INSTALL-4).

Both scripts read the installed binary's first version line *before* replacing it, so the update report is the real previous version rather than an assumption.

## PATH management

### sh: marked rc block

The installer appends exactly one marked block, and a re-run never produces a second one:

```
# >>> lca installer >>>
export PATH="$HOME/.local/bin:$PATH"
# <<< lca installer <<<
```

The block's directory is the install directory that was actually used, not the default. Target rc file by `$SHELL`: `zsh` → `~/.zshrc`; `bash` → `~/.bashrc` and, if it exists, `~/.profile`; anything else → `~/.profile`. If the install directory is already on `PATH` in the current environment, no file is edited at all — silently, because that is the ordinary re-run case. `--no-path` opts out explicitly. The installer always ends by printing the directory, the rc file it edited (or "already on PATH"), and the `source <rc>` line.

Removing the block (`--uninstall`, or a re-run that needs to rewrite it) is a line-range delete on the two marker lines, so a block a user edited by hand is still removed cleanly and nothing outside the markers is touched.

### ps1: user PATH in the registry

The install directory is appended to the *user* `Path` through `HKCU:\Environment`, never the machine `Path`, and never by rebuilding a value the installer did not read first (FR-INSTALL-9):

- Read the raw user value with `RegistryValueOptions]::DoNotExpandEnvironmentNames` and remember its `GetValueKind`, so a `REG_EXPAND_SZ` value survives the round trip as `REG_EXPAND_SZ` instead of being silently rewritten as `REG_SZ`.
- Split on `;`, drop empty entries, and append only if the directory is not already present in the user *or* the machine `Path`. Existing entries are preserved verbatim and in order; duplicates are never introduced.
- Write back with the original value kind, then broadcast `WM_SETTINGCHANGE` so a terminal opened afterwards sees it (without the broadcast, Explorer keeps serving the environment it cached at logon and "open a new terminal" would not pick the path up).

`-NoPath` skips all of it. The installer prints the same "restart your shell" line the sh script does, because a current process's environment is not rewritten in place on any Windows version.

## Uninstall

`--uninstall` / `-Uninstall`:

- deletes `<install-dir>/lca` (`lca.exe`),
- removes the marked block from **every** rc file the installer targets (`~/.zshrc`, `~/.bashrc`, `~/.profile` on sh) and the user `Path` entry on Windows — every file is checked, not just the one `$SHELL` names today, so a `bash` → `zsh` switch does not leave a stale block behind,
- prints each thing it removed, and exits 0 when there was nothing to remove (reported as such).

Nothing outside the markers, and no file the installer did not create, is modified. Sessions, credentials, configuration, and installed extensions are never touched: uninstall removes the binary, not the product's data.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success: installed, updated, uninstalled, or help printed. |
| 1 | Fetch, checksum, hash-tool, or write failure. The previous binary is untouched. |
| 2 | Unsupported platform/architecture, an unknown flag, or `--unstable` combined with `--version` (the rolling line has no pinned form). |

## Manual installation

For anyone who would rather not pipe a script into a shell (this is a legitimate preference, not a paranoia):

1. Download the asset for your platform from the release page, plus `artifacts.sha256`.
2. Verify by hand:

   ```sh
   grep ' lca-x86_64-unknown-linux-musl$' artifacts.sha256 | sha256sum -c -
   ```

   (`shasum -a 256 -c` on macOS; `Get-FileHash .\lca-x86_64-pc-windows-msvc.exe -Algorithm SHA256` compared against the matching line on Windows.)
3. Move it to a directory on your `PATH`, `chmod 755`, run `lca --version`.
4. Strong path: verify the build provenance against the repository's attestations:

   ```sh
   gh attestation verify ./lca --repo misaalanshori/lca
   ```

## Security stance

- **Binaries are not code-signed or notarized in 0.5.x.** macOS Gatekeeper will quarantine a downloaded binary; notarization is the release policy's stated direction and has not happened for these releases. The sh installer strips the quarantine attribute deliberately (`xattr -d com.apple.quarantine`) after it has verified the SHA-256 digest, because the alternative is a script that installs a binary the user then cannot run without a right-click override — and it says so rather than hiding it. The digest check, the published `artifacts.sha256`, and the in-toto provenance attestation are the trust chain until signing exists.
- **Checksum verification is mandatory in both scripts.** There is no `--skip-verify` flag and there will not be one; a script whose verification can be disabled by a flag is a script whose verification will be disabled by someone.
- **The scripts are the product surface.** They are plain POSIX sh (shellcheck `--shell=sh` clean, no bashisms) and plain PowerShell 5.1, so what a reviewer reads in the repository is what a user's shell executes: `raw.githubusercontent.com/<owner>/<repo>/main/install.sh` is the repository's own bytes, not a build output or a redirect chain.
- **Trust on first use is real.** The first run fetches a script and a binary from GitHub over TLS. A user who wants a stronger start reads the script first (`curl -fsSL <url>` to a file, read it, `sh install.sh`), and `docs/adr/0040` records why no stronger mechanism (signed script, custom domain) is in place.

## Mirror and test seam

`LCA_BASE_URL` / `-BaseUrl` exists for three callers: a mirror behind a proxy, an air-gapped host with a local release mirror, and the test suite, which points it at a fixture directory and never touches the network (FR-INSTALL-6). The tests also stub `uname` through a `PATH` prefix, so all four sh platform mappings are exercised on one machine (`tests/install/test_install_sh.sh`).

## Related documents

- `docs/adr/0040-install-and-update.md` — why this shape and not a package manager.
- `docs/release-policy.md` — the artifact matrix, the gate list (gate 10 is this installer's), attestations.
- `docs/platform-notes.md` — the per-platform PATH and quarantine details.
- `docs/testing-plan.md` section 15 — how the installer is tested.
