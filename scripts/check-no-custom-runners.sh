#!/usr/bin/env bash
#
# check-no-custom-runners.sh — enforce the no-custom-runner E2E policy.
#
# Scans tests/e2e/cases.psd1 for Runner='custom' or Script= entries.
# Any Script path not present in tests/e2e/custom-runner-allowlist.psd1
# is a violation; the script exits 1 and lists offending entries.
#
# Usage (from repo root):
#   bash scripts/check-no-custom-runners.sh
#
# Policy: use built-in runners (cpp, rust, equiv) only.  If a capability
# is missing from a built-in runner, extend the runner instead of adding
# custom script glue.
#
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
MANIFEST="$REPO_ROOT/tests/e2e/cases.psd1"
ALLOWLIST="$REPO_ROOT/tests/e2e/custom-runner-allowlist.psd1"

failed=0
violations=()

# Extract every Script='...' value from the manifest, ignoring comment lines.
while IFS= read -r script_path; do
    [ -z "$script_path" ] && continue
    # Accept the entry if its script path appears in the allowlist.
    if ! grep -qF "'$script_path'" "$ALLOWLIST" 2>/dev/null; then
        violations+=("  $script_path")
        failed=1
    fi
done < <(grep -v '^\s*#' "$MANIFEST" | grep -oP "Script\s*=\s*'\K[^']+")

if [ "${#violations[@]}" -gt 0 ]; then
    echo ''
    echo "ERROR: Disallowed custom-script E2E entries detected."
    echo "  Manifest : $MANIFEST"
    echo "  Policy   : Runner='custom' and Script= are banned."
    echo "  Fix      : use built-in runners (cpp, rust, equiv)."
    echo "             If a capability is missing, extend the runner rather than"
    echo "             adding script glue."
    echo "  Temporary: add an entry to the allowlist with Owner + Expires."
    echo "             $ALLOWLIST"
    echo ''
    echo "Violations:"
    for v in "${violations[@]}"; do echo "$v"; done
fi

if [ "$failed" -eq 0 ]; then
    count=$(grep -v '^\s*#' "$ALLOWLIST" 2>/dev/null | grep -c "Script\s*=" || true)
    echo "check-no-custom-runners: OK  (0 violations; $count temporarily allowlisted)"
fi

exit "$failed"
