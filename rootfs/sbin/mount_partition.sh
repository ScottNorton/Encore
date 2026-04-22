#!/bin/sh

# Encore boot with instant fallback.
#
# Encore (single Rust binary) replaces all stock services. The supervisor
# manages a fallback chain:
#
#   1. Promote encore_next → encore (if staged OTA exists)
#   2. Clean stale OTA if rootfs binary is newer (rootfs flash = new intended version)
#   3. Run primary binary (/lsync/encore/encore or /usr/bin/encore)
#   4. If primary crashes fast (< 5 min): quarantine it, try fallback (/usr/bin/encore)
#   5. If fallback also crashes fast: safe mode (SSH recovery)
#
# The rootfs-baked /usr/bin/encore is the ultimate safety net — it can only
# be changed by a full firmware flash, so it's always the last known-good.
#
# Safety mechanisms:
#   - Manual disable: touch /lsync/encore/disabled
#   - OTA binary: upload to /lsync/encore/encore_next, promoted at next boot

# ── 1. Immediate amp mute (prevent boot pop) ──
/usr/bin/i2c_mute 2>/dev/null && echo "mount_partition: amp muted" || echo "mount_partition: mute failed"

# ── 2. Mount filesystems ──
mount -t yaffs2 /dev/block/mtdblock4 /factory_setting
mount -t yaffs2 /dev/block/mtdblock11 /lsync

# ── 2a. One-time migration: halo → encore ──
if [ -d /lsync/halo ] && [ ! -d /lsync/encore ]; then
    mv /lsync/halo /lsync/encore
    logger -t init "Migrated /lsync/halo to /lsync/encore"
fi

# ── 2b. Self-heal /lsync if wiped ──
# /config → /lsync/misc/config, /data → /lsync/data1 (symlinks in stock rootfs).
# If these are missing, WiFi/AP won't start. Restore from factory defaults baked
# into the read-only rootfs at /usr/share/factory/.
if [ ! -d /lsync/misc/config ]; then
    echo "mount_partition: /lsync/misc/config missing — restoring factory defaults"
    mkdir -p /lsync/misc/config
    if [ -d /usr/share/factory/misc_config ]; then
        cp /usr/share/factory/misc_config/* /lsync/misc/config/
    fi
fi
if [ ! -d /lsync/data1 ]; then
    echo "mount_partition: /lsync/data1 missing — restoring factory defaults"
    mkdir -p /lsync/data1/wifi /lsync/data1/misc/bluedroid /lsync/data1/logs
    if [ -d /usr/share/factory/data1 ]; then
        cp /usr/share/factory/data1/* /lsync/data1/
    fi
fi
# Ensure directories exist even if /lsync wasn't fully wiped
mkdir -p /lsync/data1/wifi /lsync/encore

# ── 3. Infrastructure services ──
cd /sbin/ && ./wpa_supplicant_setup.sh & cd /
/sbin/start_ap.sh &
/sbin/auto_wifi_firewall.sh &
mkdir -p /data/spotify /lsync/encore

# Create TUN device node for VPN (kernel has CONFIG_TUN=y)
mkdir -p /dev/net
[ -e /dev/net/tun ] || mknod /dev/net/tun c 10 200

# ── 4. Determine Encore binaries ──
# Priority: /lsync/encore/encore (OTA) > /usr/bin/encore (rootfs-baked)
# Fallback is always the rootfs binary — last known-good release.

# 4a. Promote staged OTA immediately (supervisor can't — it blocks on the running daemon)
if [ -f /lsync/encore/encore_next ]; then
    chmod +x /lsync/encore/encore_next
    mv /lsync/encore/encore_next /lsync/encore/encore
    echo "mount_partition: promoted encore_next → encore"
fi

# 4b. Clean stale OTA binary after rootfs flash.
# A new rootfs means /usr/bin/encore is the intended version. Old OTA binaries
# on /lsync survive the flash and would shadow it. Remove them if rootfs is newer.
if [ -x /usr/bin/encore ] && [ -f /lsync/encore/encore ] && [ /usr/bin/encore -nt /lsync/encore/encore ]; then
    rm -f /lsync/encore/encore
    echo "mount_partition: removed stale OTA binary (rootfs is newer)"
fi

# 4c. Select primary and fallback
PRIMARY=""
FALLBACK=""

if [ -x /lsync/encore/encore ]; then
    PRIMARY=/lsync/encore/encore
    [ -x /usr/bin/encore ] && FALLBACK=/usr/bin/encore
elif [ -x /usr/bin/encore ]; then
    PRIMARY=/usr/bin/encore
fi

# ── 5. Manual disable check ──
if [ -f /lsync/encore/disabled ]; then
    echo "mount_partition: Encore disabled by /lsync/encore/disabled"
    PRIMARY=""
fi

# ── 6. Start Encore via supervisor, or enter recovery mode ──
if [ -n "$PRIMARY" ]; then
    echo "mount_partition: starting supervisor (primary=$PRIMARY fallback=${FALLBACK:-none})"
    /sbin/encore_supervisor.sh "$PRIMARY" "$FALLBACK" >>/tmp/supervisor.log 2>&1 &
    echo "mount_partition: supervisor started (PID $!)"
else
    echo "mount_partition: no Encore binary — device alive for SSH recovery"
    echo "mount_partition: upload binary to /lsync/encore/encore and reboot"
fi
