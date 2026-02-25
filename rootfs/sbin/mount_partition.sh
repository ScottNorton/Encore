#!/bin/sh

# Encore boot with instant fallback.
#
# Encore (single Rust binary) replaces all stock services. The supervisor
# manages a fallback chain:
#
#   1. Run primary binary (encore_next or /lsync/encore/encore)
#   2. If primary crashes fast (< 5 min): quarantine it, try fallback (/usr/bin/encore)
#   3. If fallback also crashes fast: safe mode (SSH recovery)
#   4. Stable runs (5+ min) restart automatically — not counted as crashes
#
# The rootfs-baked /usr/bin/encore is the ultimate safety net — it can only
# be changed by a full firmware flash, so it's always the last known-good.
#
# Safety mechanisms:
#   - Manual disable: touch /lsync/encore/disabled
#   - OTA binary: upload to /lsync/encore/encore_next, auto-tested next boot

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
# Priority: encore_next (staged OTA) > /lsync/encore/encore (dev) > /usr/bin/encore (baked)
# Fallback is always the rootfs binary — last known-good release.
PRIMARY=""
FALLBACK=""

if [ -f /lsync/encore/encore_next ]; then
    chmod +x /lsync/encore/encore_next
    PRIMARY=/lsync/encore/encore_next
    [ -x /usr/bin/encore ] && FALLBACK=/usr/bin/encore
elif [ -x /lsync/encore/encore ]; then
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
