#!/usr/bin/env bash
# Fail if any tracked source file exceeds MAX_LINES non-whitespace lines.
#
# Usage:
#   scripts/check-line-count.sh                  # check all tracked source files
#   scripts/check-line-count.sh <file> [<file>]  # check the given files (used by pre-commit hook)
#
# Behavior:
#   * Counts only non-whitespace lines (lines containing at least one non-space char).
#   * Threshold is MAX_LINES (default 500). Override with env var MAX_LINES=NNN.
#   * Files listed in .linecount-allow are skipped (one path per line, '#' comments allowed).
#   * Only files with source-code extensions are checked. See SOURCE_EXTS below.

set -u

MAX_LINES="${MAX_LINES:-500}"
ALLOW_FILE=".linecount-allow"

# Extensions considered source code worth gating on.
SOURCE_EXTS_REGEX='\.(rs|py|sh|ps1|psm1|js|ts|tsx|jsx|c|cc|cpp|cxx|h|hh|hpp|hxx|saw|cry|java|go|rb)$'

# Load allow-list into a newline-delimited string. Strip CRs to tolerate
# CRLF line endings on the allow file.
allow_list=""
if [ -f "$ALLOW_FILE" ]; then
    allow_list="$(tr -d '\r' < "$ALLOW_FILE" | grep -v -E '^\s*(#|$)' || true)"
fi

is_allowed() {
    local path="$1"
    [ -z "$allow_list" ] && return 1
    # Normalize backslashes (Windows) to forward slashes for comparison.
    local norm
    norm="$(printf '%s' "$path" | tr '\\' '/')"
    while IFS= read -r entry; do
        [ -z "$entry" ] && continue
        if [ "$norm" = "$entry" ]; then
            return 0
        fi
    done <<EOF
$allow_list
EOF
    return 1
}

count_nonws() {
    # Count lines containing any non-whitespace character. Using grep with
    # POSIX [:space:] makes CRLF line endings work correctly (\r is treated
    # as whitespace, so lines that are only "\r" do not count).
    grep -c '[^[:space:]]' "$1" || true
}

# Build the file list.
files=()
if [ "$#" -gt 0 ]; then
    files=("$@")
else
    # All tracked files.
    while IFS= read -r f; do
        files+=("$f")
    done < <(git ls-files)
fi

violations=0
for f in "${files[@]}"; do
    # Skip if file no longer exists (e.g. deleted but still passed as arg).
    [ -f "$f" ] || continue
    # Only check source extensions.
    echo "$f" | grep -E -i -q "$SOURCE_EXTS_REGEX" || continue
    if is_allowed "$f"; then
        continue
    fi
    count="$(count_nonws "$f")"
    if [ "$count" -gt "$MAX_LINES" ]; then
        printf '  %s: %d non-whitespace lines (limit %d)\n' "$f" "$count" "$MAX_LINES" >&2
        violations=$((violations + 1))
    fi
done

if [ "$violations" -gt 0 ]; then
    echo "" >&2
    echo "ERROR: $violations file(s) exceed the $MAX_LINES non-whitespace line limit." >&2
    echo "Refactor the file(s) above into smaller modules. Do NOT add entries to $ALLOW_FILE" >&2
    echo "without explicit reviewer approval." >&2
    exit 1
fi

exit 0
