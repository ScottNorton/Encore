#!/bin/busybox sh

modules=/lib/modules/wlan_sd8887/
insmod $modules/mlan.ko 2>/dev/null
insmod $modules/sd8xxx.ko cal_data_cfg=mrvl/WlanCalData_ext-LS9AD-20160725.conf txpwrlimit_cfg=mrvl/txpwrlimit_cfg_8887.bin cfg80211_wext=0xf auto_ds=2 fw_serial=1 sta_name="wlan" uap_name="p2p" fw_name=mrvl/sd8887_wlan_a2_p78.bin ps_mode=2 max_sta_bss=1 max_uap_bss=1 drvdbg=0x7 antenna_div=1 module_rev=22

MLAN_UP=0
echo "Wait for driver load"
while [ $MLAN_UP -lt 1 ]
do
   MLAN_UP=1
   wlan0_result=`iwconfig wlan0`
   echo $wlan0_result
   echo $wlan0_result

   if [ "x$wlan0_result" = "x" ]; then
      MLAN_UP=0
   fi
   if [ $MLAN_UP -lt 1 ]; then
     sleep 1;
   fi
done
echo "Wait for driver load done"


#Reading MAC address from Driver
mac_addr=$(cat /sys/class/net/wlan0/address)
default_mac=00:50:43:02:fe:01
if [ "$mac_addr" == "$default_mac" ]
  then
    echo "OTP MAC not programmed"
    #as OTP MAC address is missing we have to assign a random MAC address
    if [ -f /factory_setting/WIFI_MAC_ADDR ]
	then
	    # use factory setting if available
	    mac_addr=$(cat /factory_setting/WIFI_MAC_ADDR)
	    echo "MAC address Found in Factory_setting"
    	else
	    # otherwise generate a random MAC address
	    mac_addr=`( busybox dd if=/dev/urandom bs=3 count=1 2> /dev/null) | busybox hexdump -e '3/1 ":%02X"'`
	    mac_addr=00:50:43$mac_addr
	    echo "MAC address generated for wlan0"
	    echo $mac_addr
	    echo $mac_addr > /factory_setting/WIFI_MAC_ADDR
    fi
    busybox ifconfig wlan0 hw ether $mac_addr
else
    #MAC address was there in the OTP. So we have to write that to the flash
    #Reading MAC Address from flash. This is only to make sure we do not over write at every boot
     echo "OTP MAC present"
    mac_in_flash=$(cat /factory_setting/WIFI_MAC_ADDR)
    echo "mac_in_flash"
    echo $mac_in_flash
    #if OTP MAC ID is different from flash MAC ID, update MAC ID in the flash.
    if [ "$mac_addr" == "$mac_in_flash" ]
	then
	  # use factory setting if available
	  echo "MAC address already updated"
	else
	 # echo $mac_addr > /factory_setting/WIFI_MAC_ADDR
	busybox ifconfig wlan0 hw ether $mac_in_flash
	  echo "Updating MAC address from OTP"
    fi
fi

mlanutl wlan0 htcapinfo 0x800000 2

# enable network RPS(Receive Packet Steering) on wlan0 to improve WiFi stability
echo "3" >/sys/class/net/wlan0/queues/rx-0/rps_cpus
echo "3" >/sys/class/net/wlan0/queues/rx-1/rps_cpus
echo "3" >/sys/class/net/wlan0/queues/rx-2/rps_cpus
echo "3" >/sys/class/net/wlan0/queues/rx-3/rps_cpus

# wpa_supplicant config — only create from template if missing or corrupted.
# Preserves saved WiFi networks across reboots.
CONFIG_FILE_IN=/system/etc/wpa_supplicant.conf.in
CONFIG_FILE=/data/wifi/wpa_supplicant.conf

if [ ! -f ${CONFIG_FILE} ] || [ ! -s ${CONFIG_FILE} ]; then
    echo "wpa_supplicant_setup: creating config from template"
    /bin/cat ${CONFIG_FILE_IN} > ${CONFIG_FILE}
elif ! grep -q ctrl_interface ${CONFIG_FILE}; then
    echo "wpa_supplicant_setup: config corrupted, restoring from template"
    /bin/cat ${CONFIG_FILE_IN} > ${CONFIG_FILE}
else
    echo "wpa_supplicant_setup: existing config preserved"
fi

chmod 0600 ${CONFIG_FILE}
chown wifi ${CONFIG_FILE}

# Tell wpa_supplicant to re-read its config — it may have started before
# /lsync was mounted (init.rc starts it early), so the initial config read
# may have failed or used stale data.
wpa_cli -i wlan0 reconfigure >/dev/null 2>&1 || true
