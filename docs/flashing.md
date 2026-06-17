# Flashing Guide

How to put Encore on your Harman Kardon Invoke for the first time. The first flash uses a
USB cable and the speaker's built-in USB boot mode. You only do this once: after Encore is
running, every later update happens over WiFi from the web dashboard.

Nothing in this process touches the speaker's bootloader or kernel. If a flash goes wrong,
you can always enter USB boot mode again and retry or go back to stock.

## What You Need

- A Windows PC with a USB-A port (the package also includes a Linux loader, but this guide
  covers the Windows tools)
- A USB-A to USB **Mini-B** cable. The small end plugs into the speaker. Mini-B is the
  older, wider connector, not the Micro-B common on old phones.
- Harman's final firmware package, called **OTA2** (free download, see Step 1)
- A built firmware image: `build/firmware/83_IMAGE`, produced by `make firmware`
  (see the [build guide](build-guide.md))

## Step 1: Get the USB Boot Files

Harman's last public firmware update shipped as a zip containing the flashing tools, the
USB driver, and the stock firmware images. Harman no longer hosts it, but it is preserved
on the Internet Archive:

- [Harman.Kardon.INVOKE.Driver.OTA2.zip](https://archive.org/download/harman.-kardon.-invoke.-driver.-ota-2/Harman.Kardon.INVOKE.Driver.OTA2.zip)

This is the same package `make download` pulls the stock image from; for flashing you need
the rest of its contents too.

1. Download and unzip it.
2. Copy everything inside its `OTA2` folder into this repo's `uboot/` folder.
3. Keep the zip somewhere safe. Its untouched copy is also your path back to stock.

## Step 2: Install the USB Driver (first time only)

1. Plug the USB cable into the PC and the speaker. Leave the speaker's power unplugged.
2. Hold the reset button (pinhole on the back) and connect power. A new unknown device
   appears in Windows Device Manager.
3. In Device Manager, right-click the unknown device, choose "Update driver", then
   "Browse my computer", and point it at the driver folder from the unzipped package.
   Windows may require allowing an unsigned driver.
4. Unplug the speaker again.

## Step 3: Put Your Firmware in Place

1. Copy your built `build/firmware/83_IMAGE` into `uboot/`, replacing the stock `83_IMAGE`
   that came from the zip.
2. Replace the contents of `uboot/79_IMAGE` with this single line:

   ```
   #skip
   ```

   `79_IMAGE` is a command script that the bootloader runs by itself. The stock script
   tries to flash everything automatically, but automatic loading is unreliable with a
   custom image (the data can be corrupted in RAM by the time flashing starts). The
   `#skip` version makes the bootloader do nothing and wait for you to type the commands,
   which is reliable.

   The file must have Unix (LF) line endings. The safe way to create it from Git Bash or
   WSL:

   ```bash
   printf '#skip\n' > uboot/79_IMAGE
   ```

   Editing it in Windows Notepad can silently save CRLF line endings, which break the
   bootloader's command parsing.

## Step 4: Enter USB Boot Mode

1. USB cable connected, power unplugged.
2. Press and hold the reset button (light pressure, there is no click to feel).
3. While still holding reset, connect power.
4. Still holding reset, press the mic-mute button on the back **4 times**.
5. Keep holding reset until the light ring shows yellow. The speaker is now in USB boot
   mode and you can release the button.

## Step 5: Flash

1. In the `uboot/` folder, run `run.bat`. It uploads the bootloader to the speaker over
   USB and opens a terminal window.
2. Wait for the `>>` prompt to appear in the terminal.
3. Type these commands one at a time, pressing Enter after each. **Type them by hand;
   pasting can drop characters.**

   ```
   usbload 0x83 0x7000000
   tftp2nand -d <size> 0x7000000
   reset
   ```

   Replace `<size>` with the exact byte count printed at the end of `make firmware`.

4. The second command prints "writing to NAND" and takes a few minutes. Do not unplug
   anything while it runs.
5. `reset` reboots the speaker into Encore. From here, follow the First Boot steps in the
   [README](../README.md): connect to the `Invoke-XXXX` WiFi network and the setup wizard
   takes over.

## What Survives a Flash

This method rewrites only the rootfs partition. The writable settings partition (`/lsync`)
is untouched, so WiFi credentials, Encore settings, and logs survive. For a completely
fresh start, see [Factory Reset](troubleshooting.md#factory-reset).

## OTA Updates (after the first flash)

Once Encore is running you should not need the cable again.

**Via the web dashboard:** open the Update tab and upload either
`build/firmware/rootfs.squashfs` (full system update) or just the Encore binary
(`build/encore`, quicker). The speaker flashes itself and reboots.

**Via SSH:**

```bash
cat build/firmware/rootfs.squashfs | ssh -o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedKeyTypes=+ssh-rsa \
  root@<device-ip> 'cat > /run/rootfs.squashfs && flash_image rootfs /run/rootfs.squashfs && reboot'
```

The device runs Dropbear, an older SSH server, so `scp` does not work; pipe through `cat`
as shown.

**Over the USB link:** the same two methods work over the USB RNDIS connection instead of
WiFi. Plug a USB Mini-B cable from the speaker into the PC and the speaker comes up as a
USB network adapter at `10.55.55.1` (see the [USB access guide](usb-access.md)). Use
the dashboard Update tab at `http://10.55.55.1/`, or run the SSH command above with
`root@10.55.55.1` in place of `root@<device-ip>`. This is handy as a recovery path when
WiFi or the access point is unavailable. Note this is the running firmware's own gadget,
which is separate from the USB boot mode used for the first flash above.

## Going Back to Stock

Restore the original files from the OTA2 zip into `uboot/` (in particular the original
`79_IMAGE` and stock `83_IMAGE`), enter USB boot mode, and follow the instructions that
ship with Harman's package. Since the kernel and bootloader are never modified, a stock
rootfs flash returns the device to its factory state.

## If Something Goes Wrong

- The bootloader and kernel live in separate NAND partitions that this process never
  writes, so USB boot mode always remains available. Re-enter it and flash again.
- If the device doesn't boot after flashing Encore, flash the stock image instead and
  start over.
- If the terminal never reaches the `>>` prompt, check the driver installation (Step 2)
  and try a different USB port or cable.

> I have not been brave enough to try booting with no bootloader to see whether the boot
> ROM alone can recover a fully erased NAND, mainly because it sounds like a bad idea.
> Don't erase the bootloader partition.

## Files in the OTA2 Package

| File | Purpose |
|------|---------|
| `run.bat` / `run.sh` | Launches the USB loader and a terminal (Windows / Linux) |
| `usb_boot.exe` / `usb_boot` | Uploads the boot chain to the speaker over USB |
| `79_IMAGE` | Bootloader command script (replace with `#skip` for manual flashing) |
| `bootloader.img`, `sysinit.img` | Boot chain images, uploaded to RAM |
| `bcm_erom.bin.usb`, `drm_erom.img` | Security coprocessor images, uploaded to RAM |
| `81_IMAGE`, `82_IMAGE` | Stock kernel and ramdisk (never flashed by this guide) |
| `83_IMAGE` | Rootfs image (the one you replace with your build) |
| `06-09_IMAGE`, `99_IMAGE` | Partition table entries |
