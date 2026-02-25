#!/bin/sh
LOG="/tmp/start_ap.log"
echo "$(date) Waiting for p2p0 interface..." > $LOG
TRIES=0
while [ $TRIES -lt 60 ]; do
    if ifconfig p2p0 >/dev/null 2>&1; then echo "$(date) p2p0 ready" >> $LOG; break; fi
    sleep 1; TRIES=$((TRIES + 1))
done
if [ $TRIES -ge 60 ]; then echo "$(date) TIMEOUT waiting for p2p0" >> $LOG; exit 1; fi
sleep 2
# Hardcoded AP credentials — this script is the recovery path and must never fail.
# Custom AP SSID/password from config.toml is applied by Encore when it starts.
MAC=$(cat /sys/class/net/wlan0/address 2>/dev/null || cat /sys/class/net/p2p0/address 2>/dev/null || echo "")
SUFFIX=$(echo "$MAC" | sed 's/://g' | tail -c 5 | tr 'a-f' 'A-F')
[ ${#SUFFIX} -eq 4 ] && AP_SSID="Invoke-${SUFFIX}" || AP_SSID="Invoke"
AP_PASS="ridiculous"
mkdir -p /data/wifi
# Detect STA band so AP starts on the same frequency range.
# Single-radio chip can't do AP on 2.4GHz while STA is on 5GHz.
HW_MODE="g"
CHANNEL=6
IEEE80211AC=""
STA_FREQ=$(wpa_cli -i wlan0 -p /data/wifi status 2>/dev/null | grep '^freq=' | cut -d= -f2)
if [ -n "$STA_FREQ" ] && [ "$STA_FREQ" -ge 5000 ] 2>/dev/null; then
    HW_MODE="a"
    if [ "$STA_FREQ" -le 5240 ]; then CHANNEL=36
    elif [ "$STA_FREQ" -le 5320 ]; then CHANNEL=44
    elif [ "$STA_FREQ" -le 5700 ]; then CHANNEL=100
    else CHANNEL=149; fi
    IEEE80211AC="ieee80211ac=1"
    echo "$(date) STA on 5GHz (${STA_FREQ}MHz), AP using hw_mode=a ch${CHANNEL}" >> $LOG
elif [ -n "$STA_FREQ" ]; then
    echo "$(date) STA on 2.4GHz (${STA_FREQ}MHz), AP using hw_mode=g ch6" >> $LOG
else
    echo "$(date) No STA connected, AP using default hw_mode=g ch6" >> $LOG
fi
cat > /data/wifi/hostapd_ap.conf << HOSTEOF
interface=p2p0
driver=nl80211
ssid=$AP_SSID
hw_mode=$HW_MODE
channel=$CHANNEL
ieee80211n=1
${IEEE80211AC}
ctrl_interface=/data/wifi
ignore_broadcast_ssid=0
wpa=2
wpa_passphrase=$AP_PASS
wpa_key_mgmt=WPA-PSK
wpa_pairwise=CCMP
rsn_pairwise=CCMP
HOSTEOF
killall hostapd 2>/dev/null; sleep 1
echo "$(date) Starting hostapd (SSID=$AP_SSID)..." >> $LOG
/bin/hostapd /data/wifi/hostapd_ap.conf >> $LOG 2>&1 &
sleep 3
ifconfig p2p0 192.168.43.1 netmask 255.255.255.0 up
echo "$(date) p2p0 IP set to 192.168.43.1" >> $LOG
cat > /tmp/dnsmasq-ap.conf << DNSEOF
interface=p2p0
bind-interfaces
dhcp-range=192.168.43.100,192.168.43.155,255.255.255.0,12h
dhcp-option=option:router,192.168.43.1
dhcp-authoritative
no-resolv
no-poll
no-hosts
no-negcache
user=root
pid-file=/tmp/dnsmasq-ap.pid
dhcp-leasefile=/tmp/dnsmasq-ap.leases
address=/#/192.168.43.1
dhcp-option=6,192.168.43.1
dhcp-option=114,http://192.168.43.1/
DNSEOF
/bin/dnsmasq -C /tmp/dnsmasq-ap.conf >> $LOG 2>&1
touch /tmp/ap_ready
echo "$(date) AP ready: $AP_SSID at 192.168.43.1" >> $LOG
