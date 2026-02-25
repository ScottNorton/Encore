#!/bin/sh
LOG="/tmp/auto_wifi_firewall.log"
echo "$(date) auto_wifi_firewall starting..." > $LOG

# Wait for wpa_supplicant
TRIES=0
while [ $TRIES -lt 60 ]; do
    if wpa_cli -i wlan0 status >/dev/null 2>&1; then
        echo "$(date) wpa_supplicant ready after ${TRIES}s" >> $LOG; break
    fi
    sleep 1; TRIES=$((TRIES + 1))
done
if [ $TRIES -ge 60 ]; then echo "$(date) TIMEOUT waiting for wpa_supplicant" >> $LOG; exit 1; fi

# wpa_supplicant manages its own saved networks (persisted via save_config).
# WiFi credentials from config.toml are synced by Encore when it starts.
# Wait for auto-connect from saved networks.
sleep 10
STATE=$(wpa_cli -i wlan0 status 2>/dev/null | grep wpa_state | cut -d= -f2)
if [ "$STATE" = "COMPLETED" ]; then
    IP=$(wpa_cli -i wlan0 status | grep ip_address | cut -d= -f2)
    echo "$(date) WiFi connected (IP=$IP)" >> $LOG
else
    echo "$(date) No saved WiFi network — AP mode only" >> $LOG
    echo "$(date) Connect to AP and set WiFi via web dashboard" >> $LOG
fi

# Fix DNS — stock resolv.conf has nameserver 127.0.0.1 (no local resolver for wlan0)
STATE=$(wpa_cli -i wlan0 status 2>/dev/null | grep wpa_state | cut -d= -f2)
if [ "$STATE" = "COMPLETED" ]; then
    GW=$(cat /proc/net/route | awk '/wlan0.*00000000/ {print $3}' | head -1)
    if [ -n "$GW" ]; then
        GW_IP=$(printf "%d.%d.%d.%d" 0x${GW:6:2} 0x${GW:4:2} 0x${GW:2:2} 0x${GW:0:2})
        echo "nameserver $GW_IP" > /etc/resolv.conf
        echo "$(date) DNS set to gateway $GW_IP" >> $LOG
    fi
    echo "nameserver 8.8.8.8" >> /etc/resolv.conf
    echo "nameserver 8.8.4.4" >> /etc/resolv.conf
fi

echo "$(date) Activating firewall..." >> $LOG
/etc/init.d/S01firewall start >> $LOG 2>&1
echo "$(date) Done." >> $LOG
