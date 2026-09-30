#!/bin/sh
# The install.sh suite (docs/testing-plan.md section 15).
#
# Verifies: FR-INSTALL-1 FR-INSTALL-2 FR-INSTALL-3 FR-INSTALL-4 FR-INSTALL-5
# Verifies: FR-INSTALL-6 FR-INSTALL-7 FR-INSTALL-8
#
# Hermetic: a sandbox HOME, a fixture directory laid out like a release,
# LCA_BASE_URL pointed at it (file:// - the fetch seam, FR-INSTALL-6), and a
# uname stub earlier on PATH so all four platform mappings run anywhere.
# No case makes a network request.
set -u

REPO=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
INSTALLER="$REPO/install.sh"
[ -f "$INSTALLER" ] || { echo "FAIL: $INSTALLER not found" >&2; exit 1; }

PASSED=0

die() {
  printf 'FAIL: %s\n' "$*" >&2
  [ -n "${OUT:-}" ] && [ -f "$OUT" ] && sed 's/^/  | /' "$OUT" >&2
  exit 1
}

ok() {
  PASSED=$((PASSED + 1))
  printf 'ok %d - %s\n' "$PASSED" "$1"
}

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    die "no sha256 tool on this machine (sha256sum or shasum)"
  fi
}

# A fixture asset is a tiny executable that prints its own version line, in
# the shape the real binary prints it (clap's "lca 0.5.2"), so the
# installer's read-the-old-version path has something real to read.
make_asset() { # make_asset <path> <version>
  cat > "$1" <<EOF
#!/bin/sh
echo "lca $2"
EOF
  chmod +x "$1"
}

# artifacts.sha256 in the release format the real one uses:
# "<hash><two spaces><name>" per line, one line per asset in the directory.
write_checksums() { # write_checksums <dir>
  dir=$1
  out="$dir/artifacts.sha256"
  : > "$out"
  for f in "$dir"/lca-*; do
    [ -f "$f" ] || continue
    printf '%s  %s\n' "$(hash_file "$f")" "$(basename "$f")" >> "$out"
  done
}

ASSETS="lca-x86_64-unknown-linux-musl lca-aarch64-unknown-linux-musl lca-x86_64-apple-darwin lca-aarch64-apple-darwin"

write_uname_stub() { # write_uname_stub <sys> <mach>
  mkdir -p "$SANDBOX/stub"
  cat > "$SANDBOX/stub/uname" <<EOF
#!/bin/sh
case "\${1:-}" in
  -s) echo "$1" ;;
  -m) echo "$2" ;;
  *) echo "$1" ;;
esac
EOF
  chmod +x "$SANDBOX/stub/uname"
}

# Fresh sandbox per case: HOME, fixture release (latest + one pinned
# version), the uname stub, and a scratch rc file target.
setup() {
  SANDBOX=$(mktemp -d)
  OUT="$SANDBOX/out"
  HOME_DIR="$SANDBOX/home"
  mkdir -p "$HOME_DIR"
  FIX="$SANDBOX/release"
  mkdir -p "$FIX/latest/download" "$FIX/download/v9.9.9"
  for a in $ASSETS; do
    make_asset "$FIX/latest/download/$a" "0.0.0"
    make_asset "$FIX/download/v9.9.9/$a" "9.9.9"
  done
  write_checksums "$FIX/latest/download"
  write_checksums "$FIX/download/v9.9.9"
  write_uname_stub Linux x86_64
  TEST_SHELL=/bin/bash
  INSTALLED="$SANDBOX/target/lca"
}

teardown() { rm -rf "$SANDBOX"; }

# run_installer [args...]: sandboxed environment, exit status in $STATUS.
run_installer() {
  env -i \
    PATH="$SANDBOX/stub:/usr/bin:/bin:/usr/local/bin" \
    HOME="$HOME_DIR" \
    SHELL="$TEST_SHELL" \
    LCA_BASE_URL="file://$FIX" \
    USER="${USER:-test}" \
    sh "$INSTALLER" "$@" >"$OUT" 2>&1
  STATUS=$?
}

##############################################################################
# FR-INSTALL-7: platform mapping (positive half) and the unsupported pair.
##############################################################################

for pair in "Linux x86_64 lca-x86_64-unknown-linux-musl" \
            "Linux aarch64 lca-aarch64-unknown-linux-musl" \
            "Darwin x86_64 lca-x86_64-apple-darwin" \
            "Darwin arm64 lca-aarch64-apple-darwin"; do
  setup
  sys=${pair%% *}; rest=${pair#* }
  mach=${rest%% *}; asset=${rest#* }
  write_uname_stub "$sys" "$mach"
  TEST_SHELL=/bin/bash
  run_installer --install-dir "$SANDBOX/target" --no-path
  [ "$STATUS" -eq 0 ] || die "$sys/$mach: exit $STATUS, expected 0"
  [ -f "$INSTALLED" ] || die "$sys/$mach: $asset not installed"
  grep -q "9.9.9\|0.0.0" "$INSTALLED" || die "$sys/$mach: installed file is not the fixture"
  # the installed bytes are the fixture asset for that mapping
  cmp -s "$INSTALLED" "$FIX/latest/download/$asset" || die "$sys/$mach: wrong asset (want $asset)"
  ok "$sys/$mach selects $asset"
  teardown
done

setup
write_uname_stub SunOS sparc64
run_installer --install-dir "$SANDBOX/target" --no-path
[ "$STATUS" -eq 2 ] || die "unsupported platform: exit $STATUS, expected 2"
grep -q "install.ps1" "$OUT" || die "unsupported platform: message does not point at install.ps1"
grep -qi "SunOS" "$OUT" || die "unsupported platform: message does not name the pair"
ok "unsupported platform exits 2 and points at the PowerShell one-liner"
teardown

##############################################################################
# FR-INSTALL-1 / FR-INSTALL-2: verify before install, refuse on mismatch,
# and a failed download leaves an existing binary untouched.
##############################################################################

setup
run_installer --install-dir "$SANDBOX/target" --no-path
[ "$STATUS" -eq 0 ] || die "baseline install failed: exit $STATUS"
cp "$INSTALLED" "$SANDBOX/baseline"
# tamper: the recorded digest no longer matches the asset
echo "tampered" >> "$FIX/latest/download/lca-x86_64-unknown-linux-musl"
run_installer --install-dir "$SANDBOX/target" --no-path
[ "$STATUS" -eq 1 ] || die "checksum mismatch: exit $STATUS, expected 1"
grep -qi "checksum\|digest\|sha" "$OUT" || die "checksum mismatch: no refusal message"
cmp -s "$INSTALLED" "$SANDBOX/baseline" || die "checksum mismatch: existing binary was modified"
ok "checksum mismatch refuses and leaves the installed binary byte-identical"
teardown

setup
run_installer --install-dir "$SANDBOX/target" --no-path
[ "$STATUS" -eq 0 ] || die "baseline install failed: exit $STATUS"
cp "$INSTALLED" "$SANDBOX/baseline"
# failed download: base points at nothing
env -i PATH="$SANDBOX/stub:/usr/bin:/bin:/usr/local/bin" HOME="$HOME_DIR" \
  SHELL="$TEST_SHELL" LCA_BASE_URL="file://$SANDBOX/does-not-exist" \
  sh "$INSTALLER" --install-dir "$SANDBOX/target" --no-path >"$OUT" 2>&1
STATUS=$?
[ "$STATUS" -eq 1 ] || die "failed download: exit $STATUS, expected 1"
cmp -s "$INSTALLED" "$SANDBOX/baseline" || die "failed download: existing binary was modified"
[ ! -e "$SANDBOX/target/lca.part" ] || die "failed download: temp file left behind"
ok "failed download exits 1 and leaves the installed binary intact"
teardown

##############################################################################
# FR-INSTALL-1 / FR-INSTALL-3: a fresh install places an executable and
# writes exactly one marked rc block.
##############################################################################

setup
run_installer --install-dir "$SANDBOX/target"
[ "$STATUS" -eq 0 ] || die "fresh install: exit $STATUS"
[ -f "$INSTALLED" ] || die "fresh install: binary missing"
[ -x "$INSTALLED" ] || die "fresh install: binary is not executable"
find "$INSTALLED" -perm 0755 | grep -q . || die "fresh install: mode is not 755"
rc="$HOME_DIR/.bashrc"
[ -f "$rc" ] || die "fresh install: $rc not written"
blocks=$(grep -c '^# >>> lca installer >>>$' "$rc")
[ "$blocks" -eq 1 ] || die "fresh install: $blocks marked blocks in $rc, expected 1"
grep -q '^# <<< lca installer <<<$' "$rc" || die "fresh install: closing marker missing"
grep -q 'export PATH="' "$rc" || die "fresh install: PATH export missing"
grep -q "$SANDBOX/target" "$rc" || die "fresh install: block names the actual install dir"
grep -qi "source " "$OUT" || die "fresh install: output does not say how to load the rc"
# The version line clap prints already contains "lca"; the report must not
# echo it twice (the defect a real one-liner run caught on 2026-10-01).
grep -q "installed lca 0.0.0 to " "$OUT" || die "fresh install: version report is not 'installed lca 0.0.0 to <dir>'"
ok "fresh install places an executable binary and one marked rc block"
teardown

##############################################################################
# FR-INSTALL-4: re-running updates and reports old -> new, still one block.
# FR-INSTALL-3: idempotency of the marked block.
##############################################################################

setup
run_installer --install-dir "$SANDBOX/target"
[ "$STATUS" -eq 0 ] || die "first install: exit $STATUS"
# the fixture moves on: latest is now 1.2.3
for a in $ASSETS; do make_asset "$FIX/latest/download/$a" "1.2.3"; done
write_checksums "$FIX/latest/download"
run_installer --install-dir "$SANDBOX/target"
[ "$STATUS" -eq 0 ] || die "re-run: exit $STATUS"
grep -q "lca 0.0.0 -> 1.2.3" "$OUT" || die "re-run: output does not report 'lca 0.0.0 -> 1.2.3'"
blocks=$(grep -c '^# >>> lca installer >>>$' "$HOME_DIR/.bashrc")
[ "$blocks" -eq 1 ] || die "re-run: $blocks marked blocks, expected 1"
grep -q "1.2.3" "$INSTALLED" || die "re-run: binary was not replaced"
ok "re-run updates the binary, reports old -> new, and keeps one block"
teardown

##############################################################################
# FR-INSTALL-5: --uninstall removes the binary and the block, nothing else.
##############################################################################

setup
# rc files that existed before this install: --uninstall must leave them
# byte-identical (no blank line, no marker, no ownership flag).
echo 'alias ll="ls -l"' > "$HOME_DIR/.bashrc"
echo '# profile line' > "$HOME_DIR/.profile"
cp "$HOME_DIR/.bashrc" "$SANDBOX/bashrc.before"
cp "$HOME_DIR/.profile" "$SANDBOX/profile.before"
run_installer --install-dir "$SANDBOX/target"
[ "$STATUS" -eq 0 ] || die "setup install: exit $STATUS"
grep -q '^# >>> lca installer >>>$' "$HOME_DIR/.bashrc" || die "setup: no block in .bashrc"
grep -q '^# >>> lca installer >>>$' "$HOME_DIR/.profile" || die "setup: no block in .profile"
# a block left behind by an earlier shell flavor (no ownership flag), plus
# an unrelated rc line
# shellcheck disable=SC2016  # the literal $PATH expands when the rc is sourced
printf '\n# >>> lca installer >>>\nexport PATH="%s:$PATH"\n# <<< lca installer <<<\n' \
  "$SANDBOX/target" >> "$HOME_DIR/.zshrc"
echo 'alias zz="ls -1"' >> "$HOME_DIR/.zshrc"
run_installer --uninstall --install-dir "$SANDBOX/target"
[ "$STATUS" -eq 0 ] || die "--uninstall: exit $STATUS"
[ ! -e "$INSTALLED" ] || die "--uninstall: binary still present"
cmp -s "$HOME_DIR/.bashrc" "$SANDBOX/bashrc.before" || die "--uninstall: .bashrc is not byte-identical to its pre-install content"
cmp -s "$HOME_DIR/.profile" "$SANDBOX/profile.before" || die "--uninstall: .profile is not byte-identical to its pre-install content"
grep -q '^# >>> lca installer >>>$' "$HOME_DIR/.zshrc" && die "--uninstall: block left in .zshrc"
grep -q 'alias zz=' "$HOME_DIR/.zshrc" || die "--uninstall: removed a line it did not write"
grep -qi "remov" "$OUT" || die "--uninstall: output does not report what it removed"
ok "--uninstall removes the binary and every block, leaving rc files byte-identical"
teardown

setup
# No rc file at all: the one the installer created is its own residue and
# has to go with it, not sit there empty.
run_installer --install-dir "$SANDBOX/target"
[ "$STATUS" -eq 0 ] || die "setup install: exit $STATUS"
[ -f "$HOME_DIR/.bashrc" ] || die "setup: the installer did not create .bashrc"
run_installer --uninstall --install-dir "$SANDBOX/target"
[ "$STATUS" -eq 0 ] || die "--uninstall (created file): exit $STATUS"
[ ! -e "$HOME_DIR/.bashrc" ] || die "--uninstall: left behind the rc file it created"
ok "--uninstall removes an rc file it created instead of leaving it empty"
teardown

##############################################################################
# FR-INSTALL-3: --no-path leaves the rc file alone; the directory already on
# PATH is the silent case.
##############################################################################

setup
echo '# my shell' > "$HOME_DIR/.bashrc"
cp "$HOME_DIR/.bashrc" "$SANDBOX/rc.before"
run_installer --install-dir "$SANDBOX/target" --no-path
[ "$STATUS" -eq 0 ] || die "--no-path: exit $STATUS"
[ -f "$INSTALLED" ] || die "--no-path: binary missing"
cmp -s "$HOME_DIR/.bashrc" "$SANDBOX/rc.before" || die "--no-path: rc file was modified"
ok "--no-path installs and leaves the rc file byte-identical"
teardown

setup
echo '# my shell' > "$HOME_DIR/.bashrc"
cp "$HOME_DIR/.bashrc" "$SANDBOX/rc.before"
# the install directory is already on PATH: skip the edit silently
env -i PATH="$SANDBOX/target:$SANDBOX/stub:/usr/bin:/bin:/usr/local/bin" \
  HOME="$HOME_DIR" SHELL="$TEST_SHELL" LCA_BASE_URL="file://$FIX" \
  sh "$INSTALLER" --install-dir "$SANDBOX/target" >"$OUT" 2>&1
STATUS=$?
[ "$STATUS" -eq 0 ] || die "already-on-PATH: exit $STATUS"
[ -f "$INSTALLED" ] || die "already-on-PATH: binary missing"
cmp -s "$HOME_DIR/.bashrc" "$SANDBOX/rc.before" || die "already-on-PATH: rc file was modified"
ok "directory already on PATH: installs without touching the rc file"
teardown

##############################################################################
# FR-INSTALL-3: rc dispatch follows $SHELL, and the default install dir
# (LCA_INSTALL_DIR > $HOME/.local/bin) is honored.
##############################################################################

setup
TEST_SHELL=/bin/zsh
run_installer --no-path # default directory: $HOME/.local/bin
[ "$STATUS" -eq 0 ] || die "zsh dispatch: exit $STATUS"
[ -x "$HOME_DIR/.local/bin/lca" ] || die "default install dir: ~/.local/bin/lca missing"
ok "default install directory is \$HOME/.local/bin"
teardown

setup
TEST_SHELL=/bin/zsh
run_installer --install-dir "$SANDBOX/target"
[ "$STATUS" -eq 0 ] || die "zsh dispatch: exit $STATUS"
[ -f "$HOME_DIR/.zshrc" ] || die "zsh dispatch: ~/.zshrc not written"
grep -q '^# >>> lca installer >>>$' "$HOME_DIR/.zshrc" || die "zsh dispatch: block missing"
[ ! -f "$HOME_DIR/.bashrc" ] || die "zsh dispatch: wrote ~/.bashrc too"
ok "\$SHELL=zsh targets ~/.zshrc"
teardown

setup
TEST_SHELL=/usr/bin/fish
run_installer --install-dir "$SANDBOX/target"
[ "$STATUS" -eq 0 ] || die "unknown shell dispatch: exit $STATUS"
grep -q '^# >>> lca installer >>>$' "$HOME_DIR/.profile" || die "unknown shell dispatch: ~/.profile not written"
ok "an unremarkable \$SHELL falls back to ~/.profile"
teardown

setup
run_installer_env() {
  env -i PATH="$SANDBOX/stub:/usr/bin:/bin:/usr/local/bin" HOME="$HOME_DIR" \
    SHELL="$TEST_SHELL" LCA_BASE_URL="file://$FIX" \
    LCA_INSTALL_DIR="$SANDBOX/from-env" \
    sh "$INSTALLER" --no-path >"$OUT" 2>&1
  STATUS=$?
}
run_installer_env
[ "$STATUS" -eq 0 ] || die "LCA_INSTALL_DIR: exit $STATUS"
[ -x "$SANDBOX/from-env/lca" ] || die "LCA_INSTALL_DIR: install landed elsewhere"
ok "LCA_INSTALL_DIR supplies the default install directory"
teardown

##############################################################################
# FR-INSTALL-8: --version resolves that release's directory, not latest.
##############################################################################

setup
run_installer --version 9.9.9 --install-dir "$SANDBOX/target" --no-path
[ "$STATUS" -eq 0 ] || die "--version: exit $STATUS"
cmp -s "$INSTALLED" "$FIX/download/v9.9.9/lca-x86_64-unknown-linux-musl" \
  || die "--version: installed the latest asset instead of v9.9.9"
grep -q "9.9.9" "$INSTALLED" || die "--version: installed bytes are not v9.9.9"
ok "--version 9.9.9 installs the pinned release"
teardown

# The v-prefix spelling resolves to the same directory.
setup
run_installer --version v9.9.9 --install-dir "$SANDBOX/target" --no-path
[ "$STATUS" -eq 0 ] || die "--version vX.Y.Z: exit $STATUS"
cmp -s "$INSTALLED" "$FIX/download/v9.9.9/lca-x86_64-unknown-linux-musl" \
  || die "--version vX.Y.Z: wrong asset"
ok "--version v9.9.9 accepts the v prefix"
teardown

##############################################################################
# FR-INSTALL-3: bash with an existing ~/.profile gets both, exactly once.
##############################################################################

setup
echo '# profile' > "$HOME_DIR/.profile"
run_installer --install-dir "$SANDBOX/target"
[ "$STATUS" -eq 0 ] || die "bash+profile: exit $STATUS"
grep -q '^# >>> lca installer >>>$' "$HOME_DIR/.bashrc" || die "bash+profile: .bashrc block missing"
grep -q '^# >>> lca installer >>>$' "$HOME_DIR/.profile" || die "bash+profile: .profile block missing"
ok "bash writes ~/.bashrc and an existing ~/.profile"
teardown

##############################################################################
# FR-INSTALL-6: every fetch comes from LCA_BASE_URL. This whole suite runs
# offline, so any success above is the proof; this case makes it explicit by
# pointing the base at a distinct fixture and asserting the bytes.
##############################################################################

setup
ALT="$SANDBOX/mirror"
mkdir -p "$ALT/latest/download"
for a in $ASSETS; do make_asset "$ALT/latest/download/$a" "7.7.7-mirror"; done
write_checksums "$ALT/latest/download"
env -i PATH="$SANDBOX/stub:/usr/bin:/bin:/usr/local/bin" HOME="$HOME_DIR" \
  SHELL="$TEST_SHELL" LCA_BASE_URL="file://$ALT" \
  sh "$INSTALLER" --install-dir "$SANDBOX/target" --no-path >"$OUT" 2>&1
STATUS=$?
[ "$STATUS" -eq 0 ] || die "mirror: exit $STATUS"
grep -q "7.7.7-mirror" "$INSTALLED" || die "mirror: bytes did not come from the base"
ok "LCA_BASE_URL is the only source the fetch path consults"
teardown

printf '1..%d\n' "$PASSED"
printf 'install.sh: %d passed\n' "$PASSED"
