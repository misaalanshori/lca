#!/bin/sh
# LCA installer and updater - https://github.com/misaalanshori/lca
#
#   curl -fsSL https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh | sh
#
# Spec: docs/installation.md. Decision: docs/adr/0040-install-and-update.md.
# Requirements: FR-INSTALL-1..8 (FR-INSTALL-9 is install.ps1's). Tests:
# tests/install/test_install_sh.sh, gated by scripts/install-check.sh.
#
# POSIX sh, no bashisms (shellcheck --shell=sh). The same script is the
# updater: run it again and the install is replaced, verified first.
# Exit codes: 0 ok, 1 fetch/verify/write failure, 2 unsupported platform
# or a bad flag.

set -u

REPO_URL="https://github.com/misaalanshori/lca/releases"
RAW_URL="https://raw.githubusercontent.com/misaalanshori/lca/main/install.sh"
BLOCK_BEGIN='# >>> lca installer >>>'
BLOCK_END='# <<< lca installer <<<'
# Inside the block, and only when this installer created the file: the one
# thing that lets --uninstall delete a file it owns instead of leaving an
# empty rc file behind as residue.
BLOCK_OWNED='# created by the lca installer'

progname=$(basename "$0")

usage() {
  cat <<EOF
Usage: $progname [--version <X.Y.Z|vX.Y.Z>] [--install-dir <dir>] [--no-path]
                 [--unstable] [--uninstall] [--help]

Options:
  --version <ver>     install that release instead of the latest
  --unstable          install the latest build of the unstable line: a
                      rolling pre-release (ADR-0043), latest-only, so it
                      cannot be combined with --version
  --install-dir <dir> install directory (default: \${LCA_INSTALL_DIR:-\$HOME/.local/bin})
  --no-path           do not edit any shell rc file
  --uninstall         remove the installed binary and the installer's PATH block
  --help              show this help

Environment:
  LCA_INSTALL_DIR     default install directory
  LCA_BASE_URL        release base URL (mirror/test seam; file:// is copied,
                      which is what makes the test suite hermetic)

Install:
  curl -fsSL $RAW_URL | sh

Install a pinned version:
  curl -fsSL $RAW_URL | sh -s -- --version v0.6.0

Install (or update) the unstable line:
  curl -fsSL $RAW_URL | sh -s -- --unstable

Uninstall:
  curl -fsSL $RAW_URL | sh -s -- --uninstall
EOF
}

die() { # die <exit code> <message...>
  code=$1
  shift
  printf 'error: %s\n' "$*" >&2
  exit "$code"
}

# One fetch function for every download. file:// is a copy, so mirrors and
# the fixture-based tests share the same path (FR-INSTALL-6).
fetch() { # fetch <url> <dest>
  case $1 in
    file://*)
      src=${1#file://}
      [ -f "$src" ] || return 1
      cp "$src" "$2"
      ;;
    *)
      if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
      elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
      else
        printf 'error: need curl or wget to download %s\n' "$1" >&2
        return 1
      fi
      ;;
  esac
}

# sha256sum on Linux, shasum -a 256 on macOS. Absent on both is a hard
# failure: verification that can be skipped is verification (FR-INSTALL-2).
hash_file() { # hash_file <file>
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    return 1
  fi
}

# The installed binary's first version line, or nothing. clap prints
# "lca 0.6.0" for --version, so the leading name is dropped once here and
# every message downstream can say "lca <ver>" without saying it twice.
read_version() { # read_version <binary>
  [ -x "$1" ] || return 1
  line=$("$1" --version 2>/dev/null | head -n 1 | tr -d '\r')
  [ -n "$line" ] || return 1
  case $line in
    "lca "*) line=${line#lca } ;;
  esac
  printf '%s' "$line"
}

# Remove the installer's marked block, leaving every other line alone.
# If the file holds nothing but a block this installer created, the file
# itself goes: an rc file we made is residue after --uninstall.
strip_block() { # strip_block <file>
  [ -f "$1" ] || return 0
  owned=0
  grep -q "^$BLOCK_OWNED\$" "$1" && owned=1
  awk -v b="$BLOCK_BEGIN" -v e="$BLOCK_END" '
    $0 == b { skip = 1; next }
    $0 == e { skip = 0; next }
    !skip { print }
  ' "$1" >"$1.lca-tmp" || return 1
  if [ "$owned" -eq 1 ] && [ ! -s "$1.lca-tmp" ]; then
    rm -f "$1" "$1.lca-tmp"
    return 0
  fi
  mv "$1.lca-tmp" "$1"
}

has_block() { # has_block <file>
  [ -f "$1" ] && grep -q "^$BLOCK_BEGIN\$" "$1"
}

###############################################################################
# Arguments
###############################################################################

VERSION=""
INSTALL_DIR="${LCA_INSTALL_DIR:-$HOME/.local/bin}"
WANT_PATH=1
UNINSTALL=0
UNSTABLE=0

while [ $# -gt 0 ]; do
  case $1 in
    --version)
      [ $# -ge 2 ] || die 2 "--version needs a value"
      VERSION=$2
      shift 2
      ;;
    --version=*)
      VERSION=${1#--version=}
      shift
      ;;
    --install-dir)
      [ $# -ge 2 ] || die 2 "--install-dir needs a value"
      INSTALL_DIR=$2
      shift 2
      ;;
    --install-dir=*)
      INSTALL_DIR=${1#--install-dir=}
      shift
      ;;
    --no-path)
      WANT_PATH=0
      shift
      ;;
    --unstable)
      UNSTABLE=1
      shift
      ;;
    --uninstall)
      UNINSTALL=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      printf 'error: unknown flag: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

# The unstable line is "latest of the rolling line" by design: it has no
# pinned form (ADR-0043), so mixing the flags is a usage error rather than
# a silent precedence rule.
if [ "$UNSTABLE" -eq 1 ] && [ -n "$VERSION" ]; then
  die 2 "--unstable and --version cannot be combined: the unstable line is latest-only"
fi

# A relative install dir would break the rc block, which is written for
# future shells, not this one.
case $INSTALL_DIR in
  /*) ;;
  *) INSTALL_DIR=$(pwd)/$INSTALL_DIR ;;
esac

BIN_PATH="$INSTALL_DIR/lca"

###############################################################################
# Uninstall: no network, no platform detection, works anywhere.
###############################################################################

if [ "$UNINSTALL" -eq 1 ]; then
  removed=0
  if [ -e "$BIN_PATH" ] || [ -L "$BIN_PATH" ]; then
    rm -f "$BIN_PATH" || die 1 "cannot remove $BIN_PATH"
    printf 'removed binary: %s\n' "$BIN_PATH"
    removed=1
  fi
  if [ "$WANT_PATH" -eq 1 ]; then
    # Every rc this installer ever targets, so a shell change (zsh -> bash)
    # never strands a block (FR-INSTALL-5).
    for f in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.profile"; do
      if has_block "$f"; then
        strip_block "$f" || die 1 "cannot edit $f"
        printf 'removed PATH block: %s\n' "$f"
        removed=1
      fi
    done
  fi
  if [ "$removed" -eq 0 ]; then
    printf 'nothing to remove: no lca binary at %s and no installer block in %s\n' \
      "$BIN_PATH" "\$HOME/.zshrc, \$HOME/.bashrc, \$HOME/.profile"
  fi
  exit 0
fi

###############################################################################
# Platform -> release asset (FR-INSTALL-7)
###############################################################################

os=$(uname -s 2>/dev/null || printf 'unknown')
mach=$(uname -m 2>/dev/null || printf 'unknown')

case "$os:$mach" in
  Linux:x86_64) asset="lca-x86_64-unknown-linux-musl" ;;
  Linux:aarch64 | Linux:arm64) asset="lca-aarch64-unknown-linux-musl" ;;
  Darwin:x86_64) asset="lca-x86_64-apple-darwin" ;;
  Darwin:arm64 | Darwin:aarch64) asset="lca-aarch64-apple-darwin" ;;
  *)
    printf 'error: unsupported platform: %s/%s - no release asset has that name.\n' \
      "$os" "$mach" >&2
    printf 'On Windows, use the PowerShell installer instead:\n' >&2
    printf '  irm https://raw.githubusercontent.com/misaalanshori/lca/main/install.ps1 | iex\n' >&2
    exit 2
    ;;
esac

###############################################################################
# URLs (FR-INSTALL-6, FR-INSTALL-8)
###############################################################################

base="${LCA_BASE_URL:-$REPO_URL}"
if [ "$UNSTABLE" -eq 1 ]; then
  # The rolling release: one fixed directory, replaced per green commit
  # (ADR-0043). Binary and artifacts.sha256 come from the same place, so
  # verification runs unchanged.
  prefix="$base/download/unstable"
elif [ -n "$VERSION" ]; then
  version=${VERSION#v}
  prefix="$base/download/v$version"
else
  prefix="$base/latest/download"
fi

###############################################################################
# Download, verify, install (FR-INSTALL-1, FR-INSTALL-2, FR-INSTALL-4)
###############################################################################

TMP=$(mktemp -d 2>/dev/null) || die 1 "cannot create a temporary directory"
trap 'rm -rf "$TMP"' EXIT INT TERM

fetch "$prefix/artifacts.sha256" "$TMP/artifacts.sha256" ||
  die 1 "cannot fetch artifacts.sha256 from $prefix"
fetch "$prefix/$asset" "$TMP/$asset" ||
  die 1 "cannot fetch $asset from $prefix"

expected=$(awk -v n="$asset" '{ name = $2; sub(/^\*/, "", name); if (name == n) print $1 }' \
  "$TMP/artifacts.sha256" | head -n 1)
[ -n "$expected" ] || die 1 "artifacts.sha256 has no entry for $asset"

actual=$(hash_file "$TMP/$asset") ||
  die 1 "no sha256 tool found: install sha256sum (Linux) or shasum (macOS)"

if [ "$expected" != "$actual" ]; then
  printf 'error: checksum mismatch for %s\n' "$asset" >&2
  printf '  expected: %s\n  actual:   %s\n' "$expected" "$actual" >&2
  rm -f "$TMP/$asset"
  exit 1
fi

old_version=""
if [ -e "$BIN_PATH" ]; then
  old_version=$(read_version "$BIN_PATH" || true)
fi

mkdir -p "$INSTALL_DIR" || die 1 "cannot create $INSTALL_DIR"
chmod 755 "$TMP/$asset" || die 1 "cannot chmod $TMP/$asset"
mv -f "$TMP/$asset" "$BIN_PATH" 2>/dev/null ||
  { cp "$TMP/$asset" "$BIN_PATH" && rm -f "$TMP/$asset"; } ||
  die 1 "cannot place the binary at $BIN_PATH"
chmod 755 "$BIN_PATH" || die 1 "cannot chmod $BIN_PATH"

# Best effort, after verification: Gatekeeper's quarantine on an unsigned
# binary (docs/installation.md's security stance). Never fatal.
if [ "$os" = "Darwin" ]; then
  xattr -d com.apple.quarantine "$BIN_PATH" 2>/dev/null || true
fi

new_version=$(read_version "$BIN_PATH" || true)
new_version=${new_version:-unknown}

printf 'installed lca %s to %s\n' "$new_version" "$BIN_PATH"
if [ -n "$old_version" ]; then
  printf 'lca %s -> %s\n' "$old_version" "$new_version"
fi

###############################################################################
# PATH (FR-INSTALL-3)
###############################################################################

if [ "$WANT_PATH" -eq 0 ]; then
  printf 'PATH: left alone (--no-path). Restart your shell or add %s to PATH.\n' \
    "$INSTALL_DIR"
  exit 0
fi

case ":${PATH:-}:" in
  *":$INSTALL_DIR:"*)
    printf 'PATH: %s is already on PATH; no shell file edited.\n' "$INSTALL_DIR"
    exit 0
    ;;
esac

# The block carries $HOME literally when the directory lives under it, so
# the rc file survives a home-directory move (docs/installation.md).
block_dir=$INSTALL_DIR
case $INSTALL_DIR in
  "$HOME"/*)
    # shellcheck disable=SC2016  # $HOME must reach the rc file unexpanded
    block_dir='$HOME/'"${INSTALL_DIR#"$HOME"/}"
    ;;
esac

write_block() { # write_block <file>
  # Ownership survives a re-install: a file we made still carries the flag
  # after the first strip, and a file that was already there never gains it.
  owned=0
  if [ ! -f "$1" ] || grep -q "^$BLOCK_OWNED\$" "$1"; then
    owned=1
  fi
  strip_block "$1" || die 1 "cannot edit $1"
  [ -f "$1" ] || : >"$1"
  # A separating newline only when the last line is unterminated; emitting
  # one unconditionally left a blank line behind on every strip. (Command
  # substitution would eat the newline itself, so count lines instead.)
  if [ -s "$1" ] && [ "$(tail -c 1 "$1" | wc -l)" -eq 0 ]; then
    printf '\n' >>"$1"
  fi
  {
    printf '%s\n' "$BLOCK_BEGIN"
    [ "$owned" -eq 1 ] && printf '%s\n' "$BLOCK_OWNED"
    # shellcheck disable=SC2016  # $PATH must expand when the rc is sourced
    printf 'export PATH="%s:$PATH"\n' "$block_dir"
    printf '%s\n' "$BLOCK_END"
  } >>"$1" || die 1 "cannot write $1"
}

target=""
case ${SHELL:-} in
  */zsh)
    target="$HOME/.zshrc"
    write_block "$target"
    ;;
  */bash)
    target="$HOME/.bashrc"
    write_block "$target"
    if [ -f "$HOME/.profile" ]; then
      write_block "$HOME/.profile"
    fi
    ;;
  *)
    target="$HOME/.profile"
    write_block "$target"
    ;;
esac

printf 'PATH: added %s to %s\n' "$block_dir" "$target"
printf 'Restart your shell, or run: source %s\n' "$target"
