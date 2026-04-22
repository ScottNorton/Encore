#!/bin/bash
# remove_encore_next.sh — Remove staged encore_next from device if present
#
# encore_next is the OTA staging slot — the supervisor runs it in preference
# to /lsync/encore/encore. Removing it forces the supervisor back to the
# regular binary (or /usr/bin/encore as fallback).
#
# Usage (from WSL):
#   bash scripts/device/remove_encore_next.sh [ip]

. "$(dirname "$0")/../common.sh"
IP="${1:-$ENCORE_DEVICE_IP}"
SSH_OPTS="$ENCORE_SSH_OPTS -o ConnectTimeout=5"

echo "=== Remove encore_next ==="
echo "Target: $IP"
echo ""

CMD='
if [ -f /lsync/encore/encore_next ]; then
    SIZE=$(wc -c < /lsync/encore/encore_next)
    rm -f /lsync/encore/encore_next
    echo "REMOVED:${SIZE}"
else
    echo "NOT_FOUND"
fi
'

RESULT=$(sshpass -p $ENCORE_SSH_PASS ssh $SSH_OPTS root@"$IP" "$CMD" 2>&1)
RC=$?

if [ $RC -ne 0 ]; then
    echo "ERROR: SSH failed (rc=$RC)"
    echo "$RESULT"
    exit 1
fi

case "$RESULT" in
    NOT_FOUND)
        echo "encore_next not present — nothing to do."
        ;;
    REMOVED:*)
        SIZE="${RESULT#REMOVED:}"
        echo "Removed encore_next (${SIZE} bytes)."
        echo "Supervisor will now use /lsync/encore/encore or /usr/bin/encore on next run."
        ;;
    *)
        echo "Unexpected response: $RESULT"
        exit 1
        ;;
esac
