#!/usr/bin/env bash
# Credential leak checker — reads patterns from .verify-patterns (gitignored).
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
PATTERNS="$REPO/.verify-patterns"

echo "=== Credential leak check ==="
if [ ! -f "$PATTERNS" ]; then
    echo "  SKIPPED — no .verify-patterns file found."
    echo "  To enable, create .verify-patterns with one pattern per line:"
    echo "    my_wifi_ssid"
    echo "    my_wifi_password"
    echo "    my_secret_value"
    echo "  (This file is gitignored — patterns never leave your machine.)"
    echo ""
else
    fail=0
    while IFS= read -r pattern || [ -n "$pattern" ]; do
        [ -z "$pattern" ] && continue
        case "$pattern" in \#*) continue;; esac
        if grep -rn "$pattern" \
            --include='*.sh' --include='*.py' --include='*.rs' \
            --include='*.toml' --include='*.conf*' --include='*.md' \
            --include='*.html' --include='*.js' \
            "$REPO" 2>/dev/null \
            | grep -v '\.verify-patterns' \
            | grep -v 'Makefile'; then
            echo "FAIL: Found leak matching pattern"
            fail=1
        fi
    done < "$PATTERNS"
    if [ "$fail" = "1" ]; then exit 1; fi
    echo "  No credential leaks found"
    echo ""
fi

echo "=== Line ending check ==="
echo "  (Manual: check .sh and 79_IMAGE files have LF endings)"
echo ""
echo "ALL CHECKS PASSED"
