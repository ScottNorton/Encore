#!/bin/sh
#
# encore_supervisor.sh — Encore process supervisor with instant fallback.
#
# Called by mount_partition.sh:
#   /sbin/encore_supervisor.sh <primary> [fallback]
#
# Flow:
#   1. Run primary (/lsync/encore/encore or /usr/bin/encore)
#   2. If primary crashes fast (< 5 min): quarantine it, try fallback
#   3. If fallback also crashes fast: safe mode (SSH recovery)
#   4. Stable runs (5+ min) restart automatically — not counted as crashes
#
# Quarantine: bad binaries on /lsync are renamed to .bad so they aren't
# retried after reboot. This keeps the AP up — no reboot cycle.
#
# Promotion: encore_next is promoted to encore at boot time by
# mount_partition.sh (before supervisor starts). If the promoted binary
# is bad, it gets quarantined here and we fall back to /usr/bin/encore.
#
# Watchdog handoff:
#   - Supervisor owns watchdog between Encore runs (prevents reboot during restart)
#   - Encore owns watchdog while running (hang detection: if Encore stops petting, reboot)
#   - Safe mode: supervisor pets forever (device stays alive for SSH)
#
# Manual controls:
#   Disable Encore:  touch /lsync/encore/disabled  (then reboot)
#   Remove quarantine: rm /lsync/encore/encore.bad

PRIMARY="$1"
FALLBACK="${2:-}"
STABLE_SECS=300
WDT=0

# ── Watchdog helpers ──
# No background processes — pet inline to avoid fd inheritance bugs.
# BusyBox ash + POSIX: exec with failed redirect exits the shell,
# and background children inherit fds, causing EBUSY on reopen.

open_watchdog() {
    [ "$WDT" = "1" ] && return
    if exec 9>/dev/watchdog 2>/dev/null; then
        WDT=1
        printf . >&9
    fi
}

close_watchdog() {
    [ "$WDT" = "0" ] && return
    printf V >&9 2>/dev/null   # magic close — disables watchdog timer
    exec 9>&-                  # close fd
    WDT=0
}

pet_watchdog() {
    [ "$WDT" = "1" ] && printf . >&9 2>/dev/null
}

# ── Run a binary: loop on stable runs, return 1 on fast crash ──
# A stable run (>= 5 min) is restarted automatically — the binary is fine,
# it just exited (config change, OTA, etc). A fast crash (< 5 min) means
# the binary is bad — return immediately so caller can fall through.
run_encore() {
    BIN="$1"
    while true; do
        pet_watchdog
        echo "supervisor: starting $BIN"

        close_watchdog
        START=$(date +%s 2>/dev/null || echo 0)

        RUST_LOG=info "$BIN" >>/lsync/encore/encore.log 2>&1
        RC=$?

        # IMMEDIATELY reclaim watchdog — timer is counting down
        open_watchdog

        NOW=$(date +%s 2>/dev/null || echo 0)
        UP=$((NOW - START))
        echo "supervisor: $BIN exited (rc=$RC uptime=${UP}s)"

        if [ "$UP" -ge "$STABLE_SECS" ]; then
            # Stable exit (config change, OTA, etc) — restart automatically
            pet_watchdog
            sleep 2
            continue
        fi

        # Fast crash — give up on this binary
        return 1
    done
}

# ── Quarantine bad binary ──
quarantine() {
    case "$1" in
        /lsync/encore/encore)
            mv /lsync/encore/encore /lsync/encore/encore.bad 2>/dev/null
            echo "supervisor: quarantined encore → encore.bad"
            ;;
    esac
    # Signal safe mode to fallback binary
    echo 1 > /lsync/encore/safe_mode
}

# ── 1. Own watchdog immediately ──
open_watchdog
[ "$WDT" = "1" ] && echo "supervisor: watchdog open" || echo "supervisor: /dev/watchdog not available"

# ── 2. Brief safety delay (SSH window for emergency disable) ──
echo "supervisor: safety delay 5s"
pet_watchdog; sleep 5; pet_watchdog

# ── 3. Check disabled flag ──
if [ -f /lsync/encore/disabled ]; then
    echo "supervisor: disabled by /lsync/encore/disabled"
else
    echo "supervisor: starting (primary=$PRIMARY fallback=${FALLBACK:-none})"

    # ── 4. Clear safe mode flag (normal boot) ──
    rm -f /lsync/encore/safe_mode

    # ── 5. Run primary ──
    if [ -n "$PRIMARY" ]; then
        run_encore "$PRIMARY" || {
            quarantine "$PRIMARY"

            # Encore may have stopped the AP during a WiFi connect attempt.
            # Restart it so the device stays reachable regardless of crash reason.
            echo "supervisor: restarting AP after primary crash"
            /sbin/start_ap.sh >/tmp/supervisor_ap.log 2>&1 &

            # ── 6. Run fallback ──
            if [ -n "$FALLBACK" ] && [ -x "$FALLBACK" ] && [ "$PRIMARY" != "$FALLBACK" ]; then
                echo "supervisor: falling back to $FALLBACK"
                run_encore "$FALLBACK" || echo "supervisor: fallback crashed"
            fi
        }
    fi
fi

# ── 7. Safe mode — keep device alive for SSH recovery ──
echo "supervisor: safe mode"
# Ensure AP is running — Encore may have stopped it. Restart it for SSH access.
echo "supervisor: restarting AP for SSH recovery access"
/sbin/start_ap.sh >/tmp/supervisor_ap.log 2>&1 &
echo "supervisor: upload /lsync/encore/encore_next and reboot to recover"
echo "supervisor: or rm /lsync/encore/encore.bad and reboot to retry"

# Pet watchdog forever. If this loop dies, fd 9 closes without magic V,
# watchdog reboots — correct recovery for total supervisor failure.
while true; do
    pet_watchdog
    sleep 5
done
