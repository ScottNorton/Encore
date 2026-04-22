# Marvell WLAN SDK Configuration Files

The `.conf` files in this directory are Marvell WLAN driver configuration
artifacts required for the Harman Kardon Invoke's Marvell 88W8887 WiFi +
Bluetooth combo chip to function — specifically for uAP (micro access point)
mode, background scanning, country/domain regulatory data, ED-MAC control,
and related radio parameters.

## Provenance

These files are verbatim copies from the Marvell WLAN SDK, originally
distributed by Harman Kardon as part of the stock Invoke firmware on a
separate NAND partition (not within the `83_IMAGE` rootfs that Encore's
build extracts). The stock firmware's `mount_partition.sh` expected them
at `/usr/share/factory/misc_config/` at runtime; Encore's boot scripts
preserve that layout and copy them into `/lsync/misc/config/` for runtime
use by `uaputl.exe` and the Marvell driver.

The files themselves are unmodified. No Encore code has been added to or
derived from them. They are functional configuration data (parameter =
value tables) needed for hardware operation, widely redistributed across
every device shipping Marvell 88W8xxx silicon and across community
firmware projects (OpenWrt, Lineage, etc.) that support such hardware.

## Future Plans

A future release will extract these files from the user's own stock
firmware image at build time rather than shipping them in-tree, removing
any residual redistribution question. Tracked as a TODO in
`scripts/build/build_firmware.sh`.

## Rights Holder Contact

If you represent Marvell / NXP and would prefer a different arrangement
(build-time extraction, replacement, or removal), please open an issue
on the repository — we'll address it promptly.
