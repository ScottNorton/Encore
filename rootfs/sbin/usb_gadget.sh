#!/bin/sh
# usb_gadget.sh -- bring up the USB RNDIS gadget so SSH/terminal/web are reachable
# over the USB cable (10.55.55.1), independent of WiFi.
#
# Launched in the BACKGROUND from mount_partition.sh. Every step is non-fatal: if
# anything here fails the device still boots normally (Encore + WiFi recovery), so a
# bad gadget bring-up can never brick the boot.
#
# Modules: prefer the rootfs-baked /usr/lib/usbgadget (read-only, always present),
# else /lsync/usbgadget (manual/OTA staging). Built with the period Linaro 4.9.4
# toolchain (gcc-13 builds load but fault -- see memory/usb-gadget-terminal.md).
LOG=/lsync/usbg_boot.log
# usb0 is a real private /24 (not 169.254 link-local) so the device can run a
# DHCP server on it. A link-local address leaves Windows on APIPA and marks the
# RNDIS NIC an "unidentified network" with no usable config. ssh/web are at IFIP.
IFIP=10.55.55.1
IFMASK=255.255.255.0

log(){ echo "[$(cut -d' ' -f1 /proc/uptime 2>/dev/null)] $*" >> "$LOG" 2>/dev/null; }

# Hand RNDIS clients a real DHCP lease on usb0 so the PC gets a routable address
# instead of an APIPA "unidentified network". DHCP-only (port=0, no DNS), and
# crucially NO default route (option 3 empty) and NO DNS server (option 6 empty)
# -- the PC keeps its real internet (e.g. phone tether); this link is device-only.
# Reuses the stock dnsmasq the AP already uses; independent of Encore so it still
# works in SSH-recovery mode. The monitor restarts it across a stack reload.
start_usb_dhcp(){
  DNSMASQ=/usr/bin/dnsmasq; [ -x "$DNSMASQ" ] || DNSMASQ=/bin/dnsmasq
  [ -x "$DNSMASQ" ] || { log "no dnsmasq; skip usb DHCP"; return; }
  [ -f /tmp/dnsmasq-usb.pid ] && kill "$(cat /tmp/dnsmasq-usb.pid 2>/dev/null)" 2>/dev/null
  cat > /tmp/dnsmasq-usb.conf <<DNSEOF
interface=usb0
bind-interfaces
except-interface=lo
port=0
dhcp-authoritative
dhcp-range=10.55.55.10,10.55.55.50,255.255.255.0,12h
dhcp-option=3
dhcp-option=6
user=root
pid-file=/tmp/dnsmasq-usb.pid
dhcp-leasefile=/tmp/dnsmasq-usb.leases
DNSEOF
  "$DNSMASQ" -C /tmp/dnsmasq-usb.conf >>"$LOG" 2>&1 && \
    log "usb DHCP up (10.55.55.10-50, pid $(cat /tmp/dnsmasq-usb.pid 2>/dev/null))" || \
    log "usb DHCP failed to start"
}

G=/usr/lib/usbgadget
[ -f "$G/g_ether.ko" ] || G=/lsync/usbgadget
if [ ! -f "$G/g_ether.ko" ]; then log "no gadget modules in /usr/lib/usbgadget or /lsync/usbgadget; skip"; exit 0; fi

log "bring-up from $G"
# a soft fault during insmod should log, not reboot
echo 0 > /proc/sys/kernel/panic_on_oops 2>/dev/null

# Hand the shared USB PHY to the device controller: the EHCI host only has an idle
# root hub, and the host/device sides share one PHY with no OTG mux.
[ -e /sys/bus/platform/drivers/berlin-ehci/f7ed0000.usb ] && \
  echo f7ed0000.usb > /sys/bus/platform/drivers/berlin-ehci/unbind 2>/dev/null

# Load in dependency order. insmod of an already-loaded module just errors (harmless).
for m in udc-core mv_udc libcomposite g_ether; do
  insmod "$G/$m.ko" 2>>"$LOG"
done

# Wait for usb0, then put it on the link-local subnet Windows' RNDIS NIC self-assigns.
i=0
while [ ! -d /sys/class/net/usb0 ] && [ "$i" -lt 10 ]; do sleep 1; i=$((i+1)); done
if [ -d /sys/class/net/usb0 ]; then
  ifconfig usb0 "$IFIP" netmask "$IFMASK" up 2>>"$LOG"
  log "usb0 up: $(ifconfig usb0 2>/dev/null | grep -o 'inet addr:[0-9.]*')"
  start_usb_dhcp
else
  log "usb0 did not appear"
fi

# Start the self-heal monitor (graceful recovery safety net) if not already running.
MON=/sbin/usb_gadget_monitor.sh
[ -f "$MON" ] || MON=/lsync/scripts/usb_gadget_monitor.sh
if [ -f "$MON" ]; then
  if [ -f /var/run/usb_gadget_monitor.pid ] && kill -0 "$(cat /var/run/usb_gadget_monitor.pid 2>/dev/null)" 2>/dev/null; then
    log "monitor already running"
  else
    nohup sh "$MON" >/dev/null 2>&1 &
    log "monitor launched"
  fi
fi
log "bring-up done"
