# Flashing Guide

How to flash custom firmware onto the Harman Kardon Invoke using USB boot mode.

## Requirements

- A computer with a USB-A port
- USB-A to USB Mini-B cable (the device has a USB Mini-B receptacle)
- Marvell WinUSB driver installed (see below)
- Built firmware image (`firmware/83_IMAGE` or `flash/83_IMAGE`)

## Install USB Driver (First Time Only)

1. Open Device Manager
2. Plug in the Invoke while holding the reset button (pinhole on bottom)
3. A new unknown device appears — right-click → Update Driver
4. Browse to `flash/driver/` and install the Marvell WinUSB driver (may require enabling unsig. drivers)
5. Unplug the device

## Enter USB Boot Mode

1. Connect USB to computer.
2. Press and hold the reset button (pinhole on back of device, light pressure, no tactile response)
3. While holding reset, connect power, **do not release reset**
5. Quickly press the mic-mute button (back of device) **4 times** only
6. Continue holding reset until it lights yellow on the light ring
6. The device is now in USB boot mode (LED ring starts the boot animation after yellow)

## Flash

1. Open a terminal in the `flash/` directory
2. Run: `run.bat`
   - This launches `usb_boot.exe` which uploads the bootloader and opens a PuTTY terminal
3. The boot script (`79_IMAGE`) initializes and erases NAND automatically (wipes everything)
4. Wait for the U-Boot prompt (`>>`)
5. **Manually type** each command (do NOT paste — U-Boot can drop characters):
   ```
   usbload 0x83 0x7000000
   tftp2nand -d <size> 0x7000000
   reset
   ```
   Replace `<size>` with the exact file size shown at the end of `make firmware`.
6. Wait for "writing to NAND..." to complete
7. The device reboots into the new firmware

## User Data

Only data1/lsync from stock 83_IMAGE + any overlay will be on the data partitions after writing a new rootfs to NAND. Prior data0/data1 contents are removed.

To start completely fresh (wipe all config and trigger the first-boot setup wizard), see
[Factory Reset](troubleshooting.md#factory-reset).

## Important Notes

- **Type `usbload` and `tftp2nand` manually** — loading the image via 79_IMAGE is unreliable (data in RAM gets corrupted before you reach the prompt)
- **79_IMAGE must have LF line endings** — Windows CRLF breaks U-Boot command parsing
- The flash process overwrites only the rootfs partition — bootloader and kernel are preserved
- If something goes wrong, you can always re-flash with the stock 83_IMAGE.

## Recovery

If the device doesn't boot after flashing:

1. Re-enter USB boot mode (same procedure as above)
2. Flash the stock firmware image instead
3. The device will return to factory state

The bootloader and kernel are in separate NAND partitions and are never modified by this process, so the device can always be recovered via USB boot.

>I have not been brave enough to try booting with no bootloader to see if the boot rom has everything needed to recover from clean NAND, mainly because it sounds pretty dumb. Might do it someday. 

## OTA Updates (After First Flash)

Once the custom firmware is running, you can update without USB boot using `firmware/rootfs.squashfs` (built by `make firmware`).

### Via Web UI
1. Open the web dashboard → Update tab
2. Upload `firmware/rootfs.squashfs`
3. Click "Flash & Reboot"

### Via SSH
```bash
cat firmware/rootfs.squashfs | ssh -o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedKeyTypes=+ssh-rsa \
  root@<device-ip> 'cat > /run/rootfs.squashfs && flash_image rootfs /run/rootfs.squashfs && reboot'
```
Note: The device runs Dropbear (legacy SSH server) — use `cat | ssh` instead of `scp`.

## Flash Directory Contents

| File | Purpose |
|------|---------|
| `run.bat` | Launches usb_boot + PuTTY terminal |
| `usb_boot.exe` | Windows USB boot loader |
| `usb_boot_linux` | Linux USB boot loader |
| `79_IMAGE` | U-Boot command script (nandinit + nanderase) |
| `bootloader.img` | Marvell bootloader (uploaded to RAM) |
| `sysinit.img` | Early init (uploaded to RAM) |
| `bcm_erom.bin.usb` | BCM EROM (uploaded to RAM) |
| `drm_erom.img` | DRM EROM (uploaded to RAM) |
| `06-09_IMAGE` | Partition table entries |
| `81_IMAGE` | Kernel image (stock, encrypted) |
| `82_IMAGE` | Ramdisk image (stock) |
| `83_IMAGE` | Rootfs image (this is what you build) |
| `driver/` | Marvell WinUSB driver for Windows |
