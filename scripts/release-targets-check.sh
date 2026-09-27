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
readonly ZIG_TARGET_aarch64_unknown_linux_musl="aarch64-linux-musl"

host_os="$(uname -s)"
host_arch="$(uname -m)"

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

# ring forces plain gcc-mode clang for Windows AArch64 while cargo-xwin
# supplies MSVC-style flags; the committed shim translates between them. It
# is installed under a name other than `clang` on purpose: cargo-xwin caches
# `clang-cl` as a symlink to whatever `clang` it finds on PATH, so a shim
# called `clang` poisons that cache and the next run collides with it.
win_shim="$work/lca-clang-shim"
if [ "$host_os" = Linux ]; then
    install -m755 ci/clang-shim "$win_shim"
fi

check_one() {
    local target="$1"
    local log="$work/check-$target.log"
    local -a env_prefix=()
    local -a cmd

    case "$target" in
        *-unknown-linux-musl)
            if [ "$host_os" = Linux ] && [ "$host_arch" = x86_64 ] && [ "$target" = "x86_64-unknown-linux-musl" ]; then
                cmd=(cargo check -q --target "$target" -p lca-cli)
            else
                local zig_target cc_wrapper zig
                case "$target" in
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
            fi
            ;;
        *-pc-windows-msvc)
            if [ "$host_os" = Linux ]; then
                # Only the aarch64 build needs the gcc-mode shim; x86_64 uses
                # cargo-xwin's clang-cl directly (proven by the publish job).
                if [ "$target" = "aarch64-pc-windows-msvc" ]; then
                    env_prefix=("CC_${target//-/_}=$win_shim")
                fi
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
    if env "${env_prefix[@]+"${env_prefix[@]}"}" \
        timeout "$CHECK_TIMEOUT_SECONDS" "${cmd[@]}" >"$log" 2>&1; then
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
