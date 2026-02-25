# Troubleshooting

## Build Issues

### WSL `/tmp` too small

WSL2 defaults to a 32MB `/tmp` tmpfs. If you see out-of-space errors during `make firmware`:

```bash
# Expand to 512MB (must re-run after each WSL restart)
mount -o remount,size=512M /tmp

# Permanent fix — add to /etc/fstab in WSL:
# tmpfs /tmp tmpfs rw,nosuid,nodev,size=512M 0 0
```

### "mksquashfs -all-root" breaks boot

**Never** use the `-all-root` flag with mksquashfs. It sets all file UIDs/GIDs to 0, which breaks system services that depend on specific ownership. The build script uses plain `mksquashfs` without this flag.

### Rootfs too large

The NAND rootfs partition supports up to 84.8MB (84,824,064 bytes). The stock partition header is 44.2MB but `package_rootfs.py` auto-expands it to the NAND max when needed. If the SquashFS exceeds 84.8MB:
- Remove more bloat from the stock rootfs
- Check if large files were accidentally included

### CRLF line endings break U-Boot

The `79_IMAGE` file and all `.sh` scripts must have LF (Unix) line endings. Windows CRLF causes U-Boot to fail parsing commands. The `.gitattributes` file enforces this, but verify after editing on Windows:
```bash
file flash/79_IMAGE  # Should say "ASCII text", NOT "ASCII text, with CRLF line terminators"
```

### Windows encoding issues with 79_IMAGE

Always write/edit `79_IMAGE` from WSL or use an editor that saves with LF endings. If you edit it on Windows and it gets CRLF, convert it:
```bash
wsl.exe -d Ubuntu -- bash -c "sed -i 's/\r$//' /mnt/g/HKInvoke/flash/79_IMAGE"
```

### `tftp2nand` fails when automated

The `tftp2nand` command in the 79_IMAGE script sometimes fails when executed automatically by U-Boot. **Always type it manually** at the `berlin>>` prompt:
```
tftp2nand -d <size> 0x7000000
```
Replace `<size>` with the exact byte count shown at the end of `make firmware` output.

## Device Issues

### No audio output

1. Check amp mute state: The IO Expander at I2C 0x20 controls muting. Encore unmutes on startup.
2. Verify DSP firmware loaded: Check `/lsync/encore/encore.log` for DSP upload messages
3. Check ALSA: `aplay -l` should show the sound card; `amixer` shows volume controls

### WiFi won't connect

1. Check logs: `cat /tmp/auto_wifi_firewall.log`
2. Check Encore log: `cat /lsync/encore/encore.log | grep -i network`
3. Manual connect via wpa_cli:
   ```bash
   wpa_cli -i wlan0 status
   wpa_cli -i wlan0 scan
   wpa_cli -i wlan0 scan_results
   ```
4. Connect to the `Invoke-XXXX` AP and use the web UI to configure WiFi

### WiFi drops or reconnects frequently

The Invoke uses a Marvell 88W8887 single-radio combo chip. Common issues:

1. **Check signal quality**: SSH in and run:
   ```bash
   cat /proc/net/wireless          # wlan0 signal level in dBm
   wpa_cli -i wlan0 signal_poll    # detailed signal metrics
   ```
   Below -75 dBm is "Poor" — move the speaker closer to the router or add a WiFi extender.

2. **Power save causing latency**: The WiFi driver defaults to full power save mode. For testing, disable it:
   ```bash
   iwpriv wlan0 powermode 0    # disable power save (CAM = Continuously Aware Mode)
   iwpriv wlan0 deepsleep 0    # disable deep sleep
   ```
   If this fixes the drops, the issue is power management. These settings reset on reboot.

3. **AP band conflict**: The Invoke cannot operate its internal AP and STA (WiFi client) on different frequency bands. If your router is on 5 GHz and the AP is on 2.4 GHz, the driver kills the AP after ~35 seconds. Encore handles this by restarting the AP on the same band as the STA connection.

4. **5 GHz DFS channels**: Some 5 GHz channels require Dynamic Frequency Selection (DFS) radar avoidance. The Invoke may have issues with DFS channels. Try switching your router to a non-DFS channel (36, 40, 44, 48, 149, 153, 157, 161, 165).

### DNS not resolving after WiFi reconnect

Encore manages `/etc/resolv.conf` automatically — it writes the gateway IP from `/proc/net/route` plus Google DNS (8.8.8.8, 8.8.4.4) after every DHCP lease. Verify:
```bash
cat /etc/resolv.conf              # should list your gateway + 8.8.8.8
cat /proc/net/route               # check default gateway exists
ping -c1 8.8.8.8                  # raw IP connectivity (bypasses DNS)
ping -c1 google.com               # DNS resolution test
```

### DHCP fails or takes too long

Encore uses `dhcpcd` with a 15-second timeout and one automatic retry:
```bash
# Manual test:
dhcpcd wlan0 --noarp --timeout 15
# Check lease:
cat /var/run/dhcpcd-wlan0.pid     # should exist if lease active
ifconfig wlan0                    # check for assigned IP
```

Some routers need longer or respond slowly to DHCP from new clients. If consistently failing, check your router's DHCP pool isn't exhausted.

### Useful WiFi diagnostic commands

```bash
# Signal and link quality
cat /proc/net/wireless
wpa_cli -i wlan0 signal_poll

# Connection status
wpa_cli -i wlan0 status

# Driver info and capabilities
iwpriv wlan0

# DNS configuration
cat /etc/resolv.conf

# Network routes
cat /proc/net/route

# RPS (Receive Packet Steering) status
cat /sys/class/net/wlan0/queues/rx-0/rps_cpus    # should be "3" (both cores)

# WiFi scan (while connected)
wpa_cli -i wlan0 scan && sleep 3 && wpa_cli -i wlan0 scan_results
```

### LED ring not responding

1. Check Encore log for MCU I2C errors: `grep -i mcu /lsync/encore/encore.log`
2. The MCU communicates over I2C at address 0x36
3. Restart Encore: `killall encore` (supervisor will relaunch it)

### Spotify Connect not appearing

1. Check Encore log: `grep -i spotify /lsync/encore/encore.log`
2. Verify WiFi is connected (Spotify needs network access for mDNS discovery)
3. Check config: Spotify must be enabled in the web dashboard settings
4. Restart Encore: `killall encore`

### Web dashboard not loading

1. Check Encore log: `cat /lsync/encore/encore.log`
2. The web server starts with Encore — if Encore is running, the dashboard should be available
3. Try the AP: connect to `Invoke-XXXX` WiFi and access `https://encore.local` or `http://192.168.43.1`

### Can't SSH into device

- Default credentials: `root` / `ridiculous`
- Use legacy SSH options (the device has an old dropbear):
  ```bash
  ssh -o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedKeyTypes=+ssh-rsa root@encore.local
  ```
- If IP changed (DHCP), check your router's client list or connect via the AP
- **Alternative — ADB**: The stock `init.rc` activates ADB (`/sbin/adbd-root` respawns).
  Connect via USB and use `adb shell` for root access. The kernel's Android composite gadget
  also has a CDC ACM function (`/dev/ttyGS0`) — the ramdisk `inittab` runs a getty on it,
  but the main rootfs `init.rc` only lists `"adb"` in the active USB functions (ACM is
  configured but not activated). To enable USB serial, you would need to add `acm` to the
  `functions` sysfs attribute.

## HTTPS / TLS Setup

Encore generates a self-signed Certificate Authority (CA) on first boot. All TLS material is stored at `/lsync/encore/tls/`.

**How it works:**

- HTTPS is served on port 443 automatically with no configuration required.
- The CA is valid for 10 years. The server certificate is valid for 825 days and regenerates automatically when the device's IP address or hostname changes.
- Because the CA is self-signed, browsers will show a certificate warning by default.

**Getting a green padlock (trusted HTTPS):**

1. Download the CA certificate from `http://encore.local/ca.crt` (or `http://<device-ip>/ca.crt`)
2. Install it in your browser or operating system trust store:
   - **Windows**: Double-click the `.crt` file, select "Install Certificate", choose "Local Machine" or "Current User", place in "Trusted Root Certification Authorities"
   - **macOS**: Double-click the `.crt` file, add to Keychain, then set trust to "Always Trust"
   - **Linux (Chrome/Chromium)**: Go to `chrome://settings/certificates`, import under "Authorities"
   - **Firefox**: Go to Settings > Privacy & Security > Certificates > View Certificates > Authorities > Import
   - **iOS**: Download the `.crt` file in Safari, install the profile in Settings, then enable full trust under Settings > General > About > Certificate Trust Settings
   - **Android**: Settings > Security > Install from storage (varies by manufacturer)
3. After installing the CA, `https://encore.local` will show a green padlock and PWA installation will be available.

**Note:** If you factory-reset the device (clearing `/lsync`), a new CA is generated on next boot and you will need to install the new certificate.

## Factory Reset

The writable `/lsync` partition survives all flash methods (USB boot, OTA, etc.). To fully
reset the device and trigger the first-boot setup wizard, you need to clear it manually via SSH.

### Reset Encore Config Only

Removes Encore configuration, crash state, and logs. WiFi may still auto-connect from
previously saved wpa_supplicant networks.

```bash
ssh root@encore.local
rm -f /lsync/encore/config.toml /lsync/encore/disabled /lsync/encore/safe_mode /lsync/encore/encore.bad
rm -rf /lsync/encore/crashes
reboot
```

### Full Factory Reset

Removes all Encore config AND clears saved WiFi networks. The device will boot into AP-only
mode and present the setup wizard when you connect to the `Invoke-XXXX` access point.

```bash
ssh root@encore.local

# Clear Encore config
rm -f /lsync/encore/config.toml /lsync/encore/disabled /lsync/encore/safe_mode /lsync/encore/encore.bad
rm -rf /lsync/encore/crashes

# Clear saved WiFi networks from wpa_supplicant
wpa_cli -i wlan0 list_networks | tail -n +2 | cut -f1 | while read id; do
    wpa_cli -i wlan0 remove_network "$id"
done
wpa_cli -i wlan0 save_config

reboot
```

After reboot, connect to the `Invoke-XXXX` WiFi network (password: `ridiculous`). The captive
portal should open automatically with the setup wizard. If it doesn't, navigate to
`http://192.168.43.1` in your browser.

### Re-enable Encore After Safe Mode

If Encore crashed and entered safe mode, clear the quarantine state:

```bash
ssh root@encore.local
rm -f /lsync/encore/encore.bad /lsync/encore/safe_mode /lsync/encore/disabled
reboot
```

## Recovery

### Device won't boot

The device can always be recovered via USB boot mode:

1. Unplug the device
2. Hold reset (pinhole on bottom) while plugging in USB
3. Release reset, press mic-mute 4 times
4. Flash stock firmware via `flash/run.bat`

See [flashing.md](flashing.md) for the full procedure.

### Persistent storage corrupted

If `/lsync` is corrupted and causing boot issues, you can format it from U-Boot or by flashing a fresh image. The rootfs itself is read-only (SquashFS), so corruption only affects the writable yaffs2 partitions.

## Crash Diagnostics

Encore captures subsystem crashes in a persistent ring buffer at `/lsync/encore/crashes/`.
Each crash is stored as a timestamped JSON file with the subsystem name, error message,
and backtrace. The last 16 crashes are retained.

### Viewing Crashes

**Via web dashboard**: The Logs page shows crash history.

**Via REST API**:
```bash
# All crashes
curl -k https://encore.local/api/crashes

# Last crash for a specific subsystem
curl -k https://encore.local/api/crashes/bluetooth
```

**Via SSH**:
```bash
ls /lsync/encore/crashes/
cat /lsync/encore/crashes/*.json
```

### Debug Mode

Encore supports three debug modes per subsystem, configurable in `config.toml` or via the
web dashboard:

| Mode | Behavior |
|------|----------|
| `production` | Default. Failed subsystems restart automatically. |
| `hold` | Failed subsystems freeze for dashboard inspection instead of restarting. |
| `trace` | Verbose logging for the subsystem. |

Set via config:
```toml
[debug]
default_mode = "production"

[debug.overrides]
bluetooth = "hold"
```

Or at runtime via WebSocket `SetDebugMode` command.

### Hardware Debug API

For hardware investigation, Encore exposes debug endpoints (GPIO and UART probing).
These are for development use only — GPIO operations can cause CPU core pegging
if used incorrectly.

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/api/debug/gpio/state` | GET | Read all GPIO pin states |
| `/api/debug/gpio/pin/{pin}` | GET | Read specific GPIO pin |
| `/api/debug/gpio/pin/{pin}/export` | POST | Export a GPIO pin |
| `/api/debug/gpio/pin/{pin}/unexport` | POST | Unexport a GPIO pin |
| `/api/debug/uart/probe` | POST | Probe MCU UART (ttyS1) |
| `/api/debug/uart/send` | POST | Send command via UART |
| `/api/debug/uart/status` | GET | UART connection status |
| `/api/debug/gpio/init` | POST | Initialize GPIO pins for DSP SPI |
| `/api/debug/gpio/deinit` | POST | Deinitialize GPIO pins |
| `/api/debug/uart/close` | POST | Close UART connection |

See [architecture.md](architecture.md) for the full REST API reference.

## Logs

Encore logs to `/lsync/encore/encore.log` (truncated each boot, persistent across crashes within a single boot cycle).

Additional system logs:

| File | Purpose |
|------|---------|
| `/lsync/encore/encore.log` | Encore runtime log |
| `/tmp/supervisor.log` | Supervisor startup/crash log |
| `/tmp/auto_wifi_firewall.log` | WiFi connection |
| `/tmp/start_ap.log` | Access point |
| `/lsync/data1/logs/boot.log` | Persistent boot log |
| `/lsync/data1/logs/dmesg.log` | Kernel messages at boot |
