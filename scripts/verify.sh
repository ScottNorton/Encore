#!/usr/bin/env bash
# Leak checker. Run it with `make verify`.
#
# Built-in checks, no setup needed (these are the ones CI relies on):
#   - tracked key, certificate, vendor-document, and firmware-image files
#   - private-key blocks and well-known token formats in tracked text files
#   - tracked files larger than MAX_BYTES
#   - CRLF line endings in tracked shell scripts
#
# --history     also checks every commit reachable from HEAD: file names, sizes,
#               and private-key blocks.
#               Needs the full history (CI checks out with fetch-depth: 0).
# --all-refs    with --history, checks every ref in the clone instead of HEAD.
#
# Personal patterns: put one pattern per line in .verify-patterns (gitignored)
# for values only you know are private, such as your own WiFi name.
#
# VERIFY_REPO=<path> checks another checkout instead of the one this script is in.
set -euo pipefail

REPO="${VERIFY_REPO:-$(cd "$(dirname "$0")/.." && pwd)}"
PATTERNS="$REPO/.verify-patterns"
MAX_BYTES=2000000

HISTORY=0
REFS="HEAD"
for arg in "$@"; do
    case "$arg" in
        --history) HISTORY=1 ;;
        --all-refs) REFS="--all" ;;
        *) echo "usage: $0 [--history] [--all-refs]" >&2; exit 2 ;;
    esac
done

cd "$REPO"
fail=0
problem() { echo "FAIL: $*"; fail=1; }

# Never published: keys and certificates, vendor manuals, DSP loader images, and
# the combined firmware images (they contain stock Harman files).
BAD_NAMES='(\.(pem|key|p12|pfx|jks|keystore|pdf|ldr|squashfs|img)$|(^|/)83_IMAGE)'
KEY_BLOCK='-----BEGIN ([A-Z0-9]+ )*PRIVATE KEY-----'
TOKENS='(ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{40,}|AKIA[0-9A-Z]{16}|xox[baprs]-[A-Za-z0-9-]{10,}|sk-[A-Za-z0-9]{32,}|AIza[0-9A-Za-z_-]{35})'
SELF=':(exclude)scripts/verify.sh'

report() {
    # report "<what is wrong>" "<lines>"
    if [ -n "$2" ]; then
        problem "$1"
        echo "$2" | sed 's/^/    /'
    fi
}

echo "=== Tracked files that must not be published ==="
report "tracked key, certificate, vendor-document, or firmware-image files:" \
    "$(git ls-files | grep -i -E "$BAD_NAMES" || true)"

echo "=== Private keys and tokens in tracked text ==="
report "private key block in:" \
    "$(git grep -I -l -E -e "$KEY_BLOCK" -- . "$SELF" || true)"
report "something that looks like an access token in:" \
    "$(git grep -I -l -E -e "$TOKENS" -- . "$SELF" || true)"

echo "=== Large tracked files (over $MAX_BYTES bytes) ==="
report "oversized tracked files (size, path):" \
    "$(git ls-files -s \
        | sed -E 's/^[0-9]+ ([0-9a-f]+) [0-9]+\t(.*)$/\1 \2/' \
        | git cat-file --batch-check='%(objectsize) %(rest)' \
        | awk -v max="$MAX_BYTES" '$1 > max' || true)"

echo "=== Line endings in shell scripts ==="
report "shell scripts with CRLF line endings in the index:" \
    "$(git ls-files --eol -- '*.sh' 'rootfs/etc/init.d/*' | grep -E '^i/(crlf|mixed)' | cut -f2 || true)"

if [ "$HISTORY" = 1 ]; then
    echo "=== History ($REFS) ==="
    # shellcheck disable=SC2086  # REFS is deliberately split: HEAD or --all
    report "commits that add or remove a private key block:" \
        "$(git log $REFS -G"$KEY_BLOCK" --format='%h %s' -- . "$SELF" || true)"
    # shellcheck disable=SC2086
    report "key, certificate, vendor-document, or firmware-image files in history:" \
        "$(git rev-list --objects $REFS \
            | git cat-file --batch-check='%(objecttype) %(objectsize) %(rest)' \
            | awk '$1 == "blob" { $1 = ""; print }' \
            | grep -i -E "$BAD_NAMES" | sort -u || true)"
    # shellcheck disable=SC2086
    report "oversized files in history (size, path):" \
        "$(git rev-list --objects $REFS \
            | git cat-file --batch-check='%(objecttype) %(objectsize) %(rest)' \
            | awk -v max="$MAX_BYTES" '$1 == "blob" && $2 > max { $1 = ""; print }' | sort -u || true)"
fi

echo "=== Personal patterns (.verify-patterns) ==="
if [ ! -f "$PATTERNS" ]; then
    echo "  skipped, no .verify-patterns file found."
    echo "  To enable, create it with one pattern per line, for example:"
    echo "    my_wifi_ssid"
    echo "    my_wifi_password"
    echo "  (This file is gitignored, so the patterns never leave your machine.)"
else
    # Only check tracked files. Gitignored working-tree files are not published.
    tracked_files=$(git ls-files \
        '*.sh' '*.py' '*.rs' '*.toml' '*.conf*' '*.md' '*.html' '*.js' \
        2>/dev/null | grep -vE '(\.verify-patterns|Makefile)' || true)

    while IFS= read -r pattern || [ -n "$pattern" ]; do
        [ -z "$pattern" ] && continue
        case "$pattern" in \#*) continue;; esac
        if echo "$tracked_files" | xargs grep -Hn -e "$pattern" 2>/dev/null; then
            problem "found a leak matching one of your .verify-patterns"
        fi
    done < "$PATTERNS"
    [ "$fail" = 0 ] && echo "  no matches"
fi

echo ""
if [ "$fail" = 1 ]; then
    echo "VERIFY FAILED"
    exit 1
fi
echo "ALL CHECKS PASSED"
