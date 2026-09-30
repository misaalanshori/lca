#!/usr/bin/env bash
# Gate 10 (docs/release-policy.md): the installers.
#
# Runs the POSIX suite and lints both shell files. The PowerShell suite runs
# in the same CI `install` job on windows-latest, under both the 5.1 and the
# pwsh shells (docs/testing-plan.md section 15), because there is no Windows
# shell on a Linux gate host to run it under.
#
# Verifies: FR-INSTALL-1 FR-INSTALL-2 FR-INSTALL-3 FR-INSTALL-4 FR-INSTALL-5
# Verifies: FR-INSTALL-6 FR-INSTALL-7 FR-INSTALL-8
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== install.sh suite =="
sh tests/install/test_install_sh.sh

echo
echo "== shellcheck --shell=sh =="
if command -v shellcheck >/dev/null 2>&1; then
  shellcheck --shell=sh install.sh
  shellcheck --shell=sh tests/install/test_install_sh.sh
  echo "shellcheck: clean"
else
  # Named skip, not a silent pass: the CI install job installs shellcheck,
  # so the lint half of this gate is never skipped where it matters.
  echo "shellcheck: NOT INSTALLED - skipped here, run in CI (docs/testing-plan.md section 15)" >&2
fi

echo
echo "install-check: OK"
