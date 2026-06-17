#!/bin/sh
# usb_gadget_monitor.sh -- self-heal the USB RNDIS gadget if it ever wedges.
#
# Primary stability comes from the mv_udc suspend-prevention patch (the controller
# ignores host selective-suspend, so ep0 no longer desyncs). This monitor is the
# graceful-recovery safety net for anything else: it watches usb0 carrier (on this
# device an unplug keeps carrier=1 because VBUS is faked, so carrier=0 only means a
# real link failure -- never false-triggers on unplug or idle) and, if it stays down
# across two checks, reloads the stack with a 5s disconnect gap (a 1s gap is too
# short for a suspended host port to notice).
#
# Lightweight: wakes every 10s, trivial CPU, never touches the watchdog.
LOG=/lsync/usbg_monitor.log
PIDFILE=/var/run/usb_gadget_monitor.pid
IFIP=10.55.55.1
IFMASK=255.255.255.0
INTERVAL=10
DOWN_LIMIT=2
COOLDOWN=25

G=/usr/lib/usbgadget
[ -f "$G/g_ether.ko" ] || G=/lsync/usbgadget

log(){ echo "[$(cut -d' ' -f1 /proc/uptime)] $*" >> "$LOG"; }

# DHCP-only dnsmasq on usb0 -- mirrors usb_gadget.sh (the two scripts already
# duplicate IFIP/log, so keep this self-contained). NO default route / NO DNS so
# it never hijacks the PC's real internet. Restarted after a stack reload tears
# usb0 down.
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
  "$DNSMASQ" -C /tmp/dnsmasq-usb.conf >>"$LOG" 2>&1 && log "usb DHCP restarted" || log "usb DHCP restart failed"
}

reload_stack(){
  log "WEDGE (carrier down) -> reloading gadget stack from $G"
  echo 0 > /proc/sys/kernel/panic_on_oops 2>/dev/null
  # stop the DHCP server bound to the about-to-disappear usb0
  [ -f /tmp/dnsmasq-usb.pid ] && kill "$(cat /tmp/dnsmasq-usb.pid 2>/dev/null)" 2>/dev/null
  rmmod g_ether 2>/dev/null
  rmmod libcomposite 2>/dev/null
  rmmod mv_udc 2>/dev/null
  # 5s, not 1s: if Windows selective-suspended the port, a brief disconnect goes
  # unnoticed and it won't re-enumerate. A ~5s drop forces the host to notice.
  sleep 5
  insmod "$G/mv_udc.ko" 2>/dev/null
  insmod "$G/libcomposite.ko" 2>/dev/null
  insmod "$G/g_ether.ko" 2>/dev/null
  ifconfig usb0 "$IFIP" netmask "$IFMASK" up 2>/dev/null
  start_usb_dhcp
  log "reload done: udc=$(ls /sys/class/udc/ 2>&1) carrier=$(cat /sys/class/net/usb0/carrier 2>/dev/null)"
}

echo $$ > "$PIDFILE"
log "monitor started (pid $$, modules $G)"
down=0
while true; do
  sleep "$INTERVAL"
  carrier=$(cat /sys/class/net/usb0/carrier 2>/dev/null || echo 0)
  if [ "$carrier" = "1" ]; then down=0; continue; fi
  down=$((down + 1))
  log "carrier=0 (count $down/$DOWN_LIMIT)"
  if [ "$down" -ge "$DOWN_LIMIT" ]; then
    reload_stack
    down=0
    sleep "$COOLDOWN"
  fi
done
