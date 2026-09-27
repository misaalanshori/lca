#!/usr/bin/env bash
# The release targets' build gate.
#
# The six targets the release policy packages
# (docs/release-policy.md, the artifact matrix) were the last surface that
# nothing built until release time. The bug class has struck three times -
# the fuzz workspace, the bare release build, and the wasm32-wasip2 target -
# and each time a target-only break surfaced at a publish, after the
# binaries were already attached. This checks every release target compiles
# on every push, with the same toolchain the publish workflow uses.
#
# It is a *check*, not a release build: no packaging, no reproducibility
# double build (that stays in publish), no artifact naming. A run is
# composed of the targets the host can actually build:
#
#   Linux   - both linux-musl targets and both windows-msvc targets
#             (via zig cc for the cross arch, cargo-xwin for MSVC)
#   macOS   - both apple-darwin targets, natively (Apple's SDK has no
#             cross-from-Linux story; docs/platform-notes.md)
#   Windows - both windows-msvc targets, natively
#
# An explicit target list may be given as arguments (the same list the
# GH Actions matrix passes); otherwise the host's default set is used.
#
# Verifies: docs/release-policy.md (the artifact matrix), NFR-8, NFR-9,
# NFR-10; docs/testing-plan.md section 13.
set -euo pipefail

cd "$(dirname "$0")/.."

# Named constants, so a failure is legible.
readonly CHECK_TIMEOUT_SECONDS="${RELEASE_TARGETS_TIMEOUT:-900}"
readonly ZIG_TARGET_x86_64_unknown_linux_musl="x86_64-linux-musl"
readonly ZIG_TARGET_aarch64_unknown_linux_musl="aarch64-linux-musl"

host_os="$(uname -s)"

case "$host_os" in
    Linux)
        default_targets="x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-msvc aarch64-pc-windows-msvc"
        ;;
    Darwin)
        default_targets="x86_64-apple-darwin aarch64-apple-darwin"
        ;;
    MINGW* | MSYS* | CYGWIN*)
        default_targets="x86_64-pc-windows-msvc aarch64-pc-windows-msvc"
        ;;
    *)
        echo "release-targets-check: unsupported host $host_os; skipping (never fails)" >&2
        exit 0
        ;;
esac

targets="${*:-$default_targets}"

# Every check needs the target's std. Adding is cheap and idempotent.
added=""
for target in $targets; do
    if ! rustup target list --installed | grep -qx "$target"; then
        rustup target add "$target" >/dev/null
        added="$added $target"
    fi
done
if [ -n "$added" ]; then
    echo "release-targets-check: added targets:$added"
fi

# A `zig cc` wrapper for one musl triple, dropping the `--target=` cc-rs
# appends (zig takes its own target and rejects two). This is the one piece
# of cargo-zigbuild's job the gate needs, kept local so a check never needs
# a full link (the reason it stays fast enough for every push).
zig_wrapper_for() {
    local zig_target="$1" wrapper="$2"
    local zig
    zig="$(command -v zig || true)"
    if [ -z "$zig" ]; then
        for candidate in "$HOME"/tools/zig-*/zig; do
            if [ -x "$candidate" ]; then
                zig="$candidate"
                break
            fi
        done
    fi
    if [ -z "$zig" ]; then
        echo "release-targets-check: zig is required to check $zig_target (install zig 0.16)" >&2
        return 1
    fi
    cat >"$wrapper" <<EOF
#!/usr/bin/env bash
args=()
for a in "\$@"; do
    case "\$a" in --target=*) ;; *) args+=("\$a") ;; esac
done
exec "$zig" cc -target $zig_target "\${args[@]}"
EOF
    chmod +x "$wrapper"
    echo "$zig"
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# ring forces plain gcc-mode clang for Windows builds while cargo-xwin
# supplies MSVC-style flags; the committed shim translates between them.
# This mirrors the publish workflow exactly: the shim is installed as
# `clang` in a stable directory first on PATH and the two MSVC targets name
# it as their compiler. Stable on purpose - cargo-xwin caches `clang-cl` as
# a symlink to the `clang` it finds, so a temp path would dangle on the
# next run.
win_shim_dir="$PWD/target/release-targets-shim"
if [ "$host_os" = Linux ]; then
    mkdir -p "$win_shim_dir"
    install -m755 ci/clang-shim "$win_shim_dir/clang"
    PATH="$win_shim_dir:$PATH"
    export PATH
fi

check_one() {
    local target="$1"
    local log="$work/check-$target.log"
    local -a env_prefix=()
    local -a cmd

    case "$target" in
        *-unknown-linux-musl)
            # Both musl targets compile their C dependencies with `zig cc`:
            # a bare `cargo check` only works where a musl toolchain happens
            # to be installed, which the hosted runner does not have.
            local zig_target cc_wrapper zig
            case "$target" in
                x86_64-unknown-linux-musl) zig_target="$ZIG_TARGET_x86_64_unknown_linux_musl" ;;
                aarch64-unknown-linux-musl) zig_target="$ZIG_TARGET_aarch64_unknown_linux_musl" ;;
                *) echo "release-targets-check: no zig target mapping for $target" >&2; return 1 ;;
            esac
            cc_wrapper="$work/zigcc-$target"
            if ! zig="$(zig_wrapper_for "$zig_target" "$cc_wrapper")"; then
                return 1
            fi
            env_prefix=(
                "CC_${target//-/_}=$cc_wrapper"
                "AR_${target//-/_}=$zig ar"
            )
            cmd=(cargo check -q --target "$target" -p lca-cli)
            ;;
        *-pc-windows-msvc)
            if [ "$host_os" = Linux ]; then
                # The publish workflow's arrangement: both MSVC targets use
                # the shim, which is `clang` on PATH (its `real` exec picks
                # the platform clang).
                env_prefix=("CC_${target//-/_}=clang")
                cmd=(cargo xwin check -q --target "$target" -p lca-cli)
            else
                cmd=(cargo check -q --target "$target" -p lca-cli)
            fi
            ;;
        *)
            cmd=(cargo check -q --target "$target" -p lca-cli)
            ;;
    esac

    echo "release-targets-check: $target"
    local -a wrap=()
    if command -v timeout >/dev/null 2>&1; then
        wrap=(timeout "$CHECK_TIMEOUT_SECONDS")
    fi
    if env "${env_prefix[@]+"${env_prefix[@]}"}" \
        "${wrap[@]+"${wrap[@]}"}" "${cmd[@]}" >"$log" 2>&1; then
        echo "  ok: $target"
        return 0
    fi
    echo "  FAILED: $target" >&2
    tail -40 "$log" >&2
    return 1
}

status=0
for target in $targets; do
    check_one "$target" || status=1
done

if [ "$status" -ne 0 ]; then
    echo "release-targets-check: at least one release target failed to compile" >&2
    exit 1
fi
echo "release-targets-check: every assigned release target compiles"
