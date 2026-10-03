#!/bin/bash
# scripts/build/gen_tls_fallback.sh - Create the fallback TLS pair in build/tls/.
#
# The firmware makes its own HTTPS certificate on first boot and keeps it in
# /lsync/encore/tls. If that ever fails, it loads cert.pem and key.pem from
# /usr/share/encore/tls instead, and if those are missing too it makes a
# throwaway certificate in memory. This script creates the pair for the middle
# step.
#
#   Output:  build/tls/cert.pem and build/tls/key.pem (set TLS_DIR to change it)
#   Existing files are never overwritten, so you can put your own cert.pem and
#   key.pem there first. They must be a PEM certificate and its PEM private
#   key, and the certificate should cover the names and addresses you browse to.
#
# build/ is git-ignored. The private key must not be committed or published.
# build_firmware.sh runs this script and installs the pair into the image.
# See "TLS fallback pair" in docs/build-guide.md.
set -euo pipefail

# Git Bash would otherwise turn the leading "/" of openssl's -subj value into a path.
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TLS_DIR="${TLS_DIR:-$REPO_DIR/build/tls}"
CERT="$TLS_DIR/cert.pem"
KEY="$TLS_DIR/key.pem"

if ! command -v openssl >/dev/null 2>&1; then
    echo "ERROR: openssl is required to create the TLS fallback pair" >&2
    exit 1
fi

mkdir -p "$TLS_DIR"

# True when the key belongs to the certificate. Works for RSA, EC and Ed25519
# keys in any of the usual PEM layouts.
pair_matches() {
    local cert_pub key_pub
    cert_pub="$(openssl x509 -in "$CERT" -noout -pubkey 2>/dev/null | openssl sha256)" || return 1
    key_pub="$(openssl pkey -in "$KEY" -pubout 2>/dev/null | openssl sha256)" || return 1
    [ -n "$cert_pub" ] && [ "$cert_pub" = "$key_pub" ]
}

if [ -s "$CERT" ] && [ -s "$KEY" ]; then
    if ! pair_matches; then
        echo "ERROR: $KEY is not the private key for $CERT (or one of them is not a PEM file)." >&2
        echo "       Fix the files, or remove both to have new ones created." >&2
        exit 1
    fi
    echo "TLS fallback pair: keeping the existing files in $TLS_DIR"
    exit 0
fi
if [ -e "$CERT" ] || [ -e "$KEY" ]; then
    echo "ERROR: $TLS_DIR has only one of cert.pem and key.pem." >&2
    echo "       Add the missing file, or remove the other one to start over." >&2
    exit 1
fi

# Only reached when neither file exists, so cleanup can never remove a pair
# that was there before.
CREATED=0
cleanup() {
    if [ "$CREATED" != 1 ]; then
        rm -f "$CERT" "$KEY"
    fi
}
trap cleanup EXIT

echo "TLS fallback pair: creating $CERT and $KEY"
umask 077
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout "$KEY" -out "$CERT" -days 3650 \
    -subj "/CN=encore.local" \
    -addext "subjectAltName=DNS:encore.local,DNS:hkinvoke.local,IP:192.168.43.1,IP:10.55.55.1" \
    -addext "basicConstraints=critical,CA:FALSE" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=serverAuth"

# The certificate and the key must belong together.
if ! pair_matches; then
    echo "ERROR: the new certificate does not match the new key" >&2
    exit 1
fi

chmod 600 "$KEY"
chmod 644 "$CERT"
CREATED=1
echo "TLS fallback pair: done. Keep $KEY private; it is git-ignored."
