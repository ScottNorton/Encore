#!/bin/sh
# SoC peripheral probe script for Harman Kardon Invoke
# Run via SSH on the device

echo "============================================"
echo "  Harman Kardon Invoke SoC Peripheral Probe"
echo "  $(date)"
echo "============================================"

echo ""
echo "=== BLUETOOTH ==="
echo "--- /sys/class/bluetooth ---"
ls -la /sys/class/bluetooth/ 2>&1
echo "--- hciconfig ---"
hciconfig -a 2>&1
echo "--- hcitool (if present) ---"
which hcitool 2>&1 && hcitool dev 2>&1
echo "--- BT kernel modules ---"
lsmod | grep -i bt 2>&1
echo "--- BT module files ---"
find /lib/modules/ -name '*bt*' -o -name '*bluetooth*' 2>/dev/null
echo "--- BT firmware files ---"
ls -la /lib/firmware/mrvl/*bt* 2>/dev/null
ls -la /lib/firmware/*bt* 2>/dev/null
echo "--- Kernel config BT ---"
if [ -f /proc/config.gz ]; then
    zcat /proc/config.gz 2>/dev/null | grep -i CONFIG_BT
else
    echo "/proc/config.gz not found"
fi

echo ""
echo "=== I2C BUS 0 (full scan) ==="
i2cdetect -y 0 2>&1

echo ""
echo "=== I2C BUS 1 ==="
if [ -e /dev/i2c-1 ]; then
    echo "/dev/i2c-1 exists"
    i2cdetect -y 1 2>&1
else
    echo "/dev/i2c-1 does NOT exist"
fi

echo ""
echo "=== I2C BUS 2+ ==="
ls -la /dev/i2c-* 2>&1

echo ""
echo "=== TEMPERATURE SENSOR (0x48) ==="
echo "--- Register 0x00 (temp, word) ---"
i2cget -y 0 0x48 0x00 w 2>&1
echo "--- Register 0x01 (config) ---"
i2cget -y 0 0x48 0x01 2>&1

echo ""
echo "=== SPI DEVICES ==="
ls -la /dev/spi* 2>&1

echo ""
echo "=== UART DEVICES ==="
ls -la /dev/ttyS* 2>&1
ls -la /dev/ttyUSB* 2>/dev/null
ls -la /dev/ttyACM* 2>/dev/null

echo ""
echo "=== USB ==="
echo "--- USB devices ---"
lsusb 2>&1 || ls /sys/bus/usb/devices/ 2>&1
echo "--- USB gadget/OTG ---"
ls /sys/class/udc/ 2>/dev/null
cat /sys/class/udc/*/state 2>/dev/null

echo ""
echo "=== GPIO ==="
echo "--- Exported GPIOs ---"
ls /sys/class/gpio/ 2>&1
echo "--- GPIO chip info ---"
for chip in /sys/class/gpio/gpiochip*; do
    if [ -d "$chip" ]; then
        echo "$chip: base=$(cat $chip/base) ngpio=$(cat $chip/ngpio) label=$(cat $chip/label)"
    fi
done

echo ""
echo "=== KERNEL MODULES (all loaded) ==="
lsmod 2>&1

echo ""
echo "=== KERNEL CONFIG (interesting bits) ==="
if [ -f /proc/config.gz ]; then
    zcat /proc/config.gz 2>/dev/null | grep -E 'CONFIG_BT|CONFIG_HDMI|CONFIG_SND_SOC|CONFIG_SPI|CONFIG_I2C|CONFIG_USB_GADGET|CONFIG_WATCHDOG|CONFIG_RTC|CONFIG_HWMON|CONFIG_THERMAL|CONFIG_PWM' | grep -v '^#'
else
    echo "/proc/config.gz not found, trying /boot/config*"
    cat /boot/config* 2>/dev/null | grep -E 'CONFIG_BT|CONFIG_HDMI' | head -20
fi

echo ""
echo "=== THERMAL ZONES ==="
for tz in /sys/class/thermal/thermal_zone*; do
    if [ -d "$tz" ]; then
        echo "$tz: type=$(cat $tz/type 2>/dev/null) temp=$(cat $tz/temp 2>/dev/null)"
    fi
done

echo ""
echo "=== HWMON (hardware monitors) ==="
for hw in /sys/class/hwmon/hwmon*; do
    if [ -d "$hw" ]; then
        echo "$hw: name=$(cat $hw/name 2>/dev/null)"
        cat $hw/temp1_input 2>/dev/null && echo "  temp1_input: $(cat $hw/temp1_input 2>/dev/null)"
    fi
done

echo ""
echo "=== WATCHDOG ==="
ls -la /dev/watchdog* 2>&1

echo ""
echo "=== RTC ==="
ls -la /dev/rtc* 2>&1
ls /sys/class/rtc/ 2>&1

echo ""
echo "=== PWM ==="
ls /sys/class/pwm/ 2>&1

echo ""
echo "=== MARVELL WIFI/BT DRIVER DETAILS ==="
echo "--- sd8xxx params ---"
ls /sys/module/sd8xxx/parameters/ 2>/dev/null && \
  for p in /sys/module/sd8xxx/parameters/*; do
    echo "  $(basename $p)=$(cat $p 2>/dev/null)"
  done
echo "--- mlan params ---"
ls /sys/module/mlan/parameters/ 2>/dev/null

echo ""
echo "=== /proc/mwlan ==="
ls /proc/mwlan/ 2>/dev/null
cat /proc/mwlan/config 2>/dev/null | head -30

echo ""
echo "=== DEVICE TREE (if accessible) ==="
if [ -d /proc/device-tree ]; then
    echo "Device tree found"
    ls /proc/device-tree/ 2>&1
    find /proc/device-tree -name 'compatible' -exec sh -c 'echo "{}:"; cat {} 2>/dev/null; echo' \; 2>/dev/null | head -80
else
    echo "No /proc/device-tree"
fi

echo ""
echo "============================================"
echo "  Probe complete"
echo "============================================"
