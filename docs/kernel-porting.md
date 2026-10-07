# Kernel Porting: Linux 6.1 LTS on BG2CDP

## Overview

The Harman Kardon Invoke runs Linux 3.8.13 (Marvell vendor kernel from 2013). This document tracks
the effort to port Linux 6.1 LTS to the Marvell BG2CDP (88DE3006) SoC.

**Why 6.1 LTS?**
- Long-term support through December 2026
- Identical Berlin SoC support to newer kernels (BG2CDP was never upstreamed)
- Stable driver ABI for vendor driver ports

**What we gain:**
- Modern kernel security (ASLR, stack protector, seccomp)
- Working iptables/nftables (stock kernel is too old for Encore's firewall needs)
- conntrack support (currently unavailable — stock lacks the module)
- kexec support (enables kernel self-updates without reflashing)
- Modern driver framework (DT-based, upstream I2C/GPIO/WDT drivers)
- SMP with HOTPLUG_CPU (stock has SMP but no hotplug — blocks kexec)

## Boot Security — Findings

### What I Thought
The kernel was believed to be "OTP-encrypted" — AES encryption with device-unique keys
burned into silicon, making kernel replacement impossible.

### What's True?
The bootloader uses **both AES encryption AND RSA signature verification**:

1. **Boot ROM** (in silicon) loads bootloader from NAND, verifies against OTP-fused key hash
2. **Bootloader** reads kernel image from NAND — it is AES-encrypted (entropy = 8.00 bits/byte)
3. Bootloader sends kernel to **BCM security coprocessor** for decryption + RSA verification
4. If valid → kernel is decrypted to RAM and boots. If not → hangs silently.

**Confirmed by direct analysis of mtd9 (bootimgs partition):**
- Kernel payload has maximum entropy (8.00 bits/byte = encrypted)
- Ramdisk (initramfs) in the same partition is plaintext gzip (1f 8b magic, NOT encrypted)
- This means encryption is per-sub-image, not whole-partition

### Evidence
- **Chromecast tools** (`tchebb/chromecast-tools`): stripping the Marvell crypto header from
  81_IMAGE reveals a standard boot image underneath — kernel data is **plaintext, not
  AES-encrypted**
- **Chromecast Gen1** (88DE3005 - similar Marvell chip): `VerifyImage()` return value was never
  checked → allowed unsigned boot. Bug patched on newer devices.
- **Google Home Mini** (same 88DE3006): researcher confirmed "bootloader and kernel partitions
  are part of the chain of trust, cryptographically verified" — used YAFFS2 exploit instead
  of replacing kernel
- **Vendor kernel .config**: ZERO secure boot options (CONFIG_SECURE_BOOT, CONFIG_TRUSTED_BOOT
  absent)
- **Chromecast bootloader source** (GPL): shows `bcm_image_verify()` sends
  `BCM_PI_IMAGE_VERIFY` to security coprocessor. Has both AES and RSA key store types, but
  encryption is **flag-controlled** (optional per-image)

### Key Distinction
- The kernel IS AES-encrypted on NAND (confirmed by entropy analysis of mtd9)
- The ramdisk is NOT encrypted (plain gzip in the same partition)
- The encryption is per-image, controlled by the `01 00 00 00` flag in each sub-image header
- The BCM security coprocessor handles both decryption and verification
- Chromecast-tools documents a "key ID" field in the crypto header: `0x02` for NAND, `0x82` for USB

### OTP Fuse Status
| OTP Field | Value | Meaning |
|-----------|-------|---------|
| DIS USB BOOT | 0 | USB boot enabled (always recoverable) |
| MRVL/CUST SIGN | 0000 | No signature enforcement in OTP |
| OTP LOCK | 0 | Not locked |

**Note:** Despite OTP showing "no signature enforcement", the bootloader firmware itself
contains the verification code. The OTP bits may control a different enforcement level
(e.g., debug vs production), or the bootloader may always verify regardless of OTP state.
Testing is required.

### Where Is the RSA Key Hash Stored?

**Hypothesis: cenv (mtd13) may contain the RSA key hash. Needs verification.**

Evidence:
- **Direct observation** (2026-02-22): cenv partition dumped from device, contains a
  hash-length value (purpose unknown — could be RSA key hash, could be something else),
  plus default device/WiFi configuration variables — standard U-Boot environment format
- OTP fuse status shows `MRVL/CUST SIGN = 0000` — no OTP-level signature enforcement
- Yet the bootloader still verifies kernel signatures from NAND
- `fw_env.config` on the stock rootfs confirms **mtd13 (cenv) and mtd14 (senv) are U-Boot
  environment partitions** (key=value format, accessible via `fw_printenv`/`fw_setenv`)
- mtd1 (pre-bootloader) also listed as env source
- Vendor kernel source has ZERO references to cenv/mtd13 — bootloader-only partition
- The partition names "common environment" and "secure environment" suggest access control —
  senv may only be readable from TrustZone/secure world

**New finding (2026-02-23): `flash_custk` writes RSA keys to pre-bootloader, not cenv.**
The stock rootfs contains `flash_custk` — a Google Chromecast ("Eureka") tool built from
`flash_custk_salami.c` by `lcwu@google`. Usage: `flash_custk <board_name> <custk0> [custk1]`.
It writes customer signing keys into the **pre-bootloader partition (mtd1)**, not cenv.
This is how HK provisioned their RSA signing keys. The hash-length value observed in cenv
may be a secondary key, a firmware hash, or unrelated to RSA verification entirely.

**WARNING**: A kexec test nearly overwrote cenv from position 0. Writing to mtd13
without preserving the existing data would destroy the RSA key hash AND device config,
potentially soft-bricking the boot chain. The partial write (dd seek=16) failed because NAND
requires full erase-block writes (128KB).

**U-Boot environment format:**
- Bytes 0–3: CRC32 of data
- Byte 4 (optional): flags (for redundant env)
- Bytes 5+: null-terminated `key=value\0key=value\0...\0\0`
- Each partition is 128KB (0x20000)

**TODO:** Dump mtd13 (cenv) and mtd14 (senv) from device and parse:
```bash
# On device:
nanddump -f /tmp/cenv.bin /dev/mtd13
nanddump -f /tmp/senv.bin /dev/mtd14
# Then transfer and parse with fw_printenv or hexdump
```

If the RSA key hash IS in cenv (writable NAND), it could potentially be:
1. **Replaced** with our own key → sign custom kernels for direct NAND boot
2. **Cleared** → disable signature enforcement entirely
3. **Left alone** → continue using kexec bypass (current approach)

If confirmed, kernel replacement on NAND may be possible without hardware modification,
just by updating the signing key in cenv. However, this is high-risk — a bad cenv write
could soft-brick the device (recovery only via USB boot mode, if the bootloader itself still works).

**Next steps** (when device is online):
1. Dump cenv cleanly: `nanddump -f /tmp/cenv.bin /dev/mtd13`
2. Parse with `fw_printenv` or the dump script at `scripts/device/dump_uboot_env.sh`
3. Identify the exact key hash variable name and format
4. Back up cenv BEFORE any modification attempts
5. Research whether the bootloader reads the key hash from cenv directly or from senv

## Hardware Summary

| Component | Detail |
|-----------|--------|
| **SoC** | Marvell BG2CDP 88DE3006 (silicon revision B0, per `berlin_config_hw.xml`) |
| **CPU** | Dual-core ARM Cortex-A7 @ 1.3 GHz |
| **RAM** | 512 MB DDR3 |
| **NAND** | 512 MB Toshiba (single chip, flash ID da98). Page 2048, OOB 64, 128KB erase blocks (single-plane) |
| **WiFi/BT** | Marvell 88W8887 (SDIO), dual-band 802.11ac + BT 4.1 |
| **Audio DSP** | AD91210Z (Analog Devices ADSP-21489 SHARC), SPI upload |
| **Audio DAC** | PCM5121 (I2C 0x4C) → TPA3116 Class-D amp → 3x speakers |
| **MCU** | TI MSP430FR5739 (I2C 0x36), 16KB FRAM |
| **LEDs** | 15 RGB total (13 controlled by MCU, 2 didn't touch) |
| **Watchdog** | DesignWare WDT @ 0xF7FC2000 |
| **Timer** | ARM timer (armv7-timer) @ 25 MHz |

## NAND Partition Map (from `/proc/mtd`)

| MTD | Size | Name | Notes |
|-----|------|------|-------|
| mtd0 | 128K | block0 | Boot block |
| mtd1 | 1M | pre-bootloader | First-stage bootloader |
| mtd2 | 2M | post-bootloader | U-Boot |
| mtd3 | 2M | postbootloaderB | U-Boot backup |
| mtd4 | 5M | factory_setting | YAFFS2, calibration |
| mtd5 | 5M | tz_en | TrustZone |
| mtd6 | 1M | tz_en-B | TrustZone backup |
| mtd7 | 10M | bootimgs_B | Kernel+ramdisk backup |
| mtd8 | 5M | bsl | Boot status log? |
| mtd9 | 10M | bootimgs | **Kernel (encrypted) + ramdisk (gzip) + metadata** |
| mtd10 | 90M | rootfs | SquashFS (our custom firmware) |
| mtd11 | 123M | app | YAFFS2, mounted as `/lsync` |
| mtd12 | 128K | fw_stat | Firmware status |
| mtd13 | 128K | cenv | Common environment |
| mtd14 | 128K | senv | Secure environment |
| mtd15 | 256M | mv_nand | Whole NAND device |

### bootimgs (mtd9) Internal Layout

| Offset | Size | Content | Encrypted? |
|--------|------|---------|------------|
| 0x000000 | 0x1000 | Image header (load addr, sizes, cmdline path) | No |
| 0x020000 | ~4.9MB | Kernel (3x1024 signing headers + AES payload) | **Yes** |
| 0x4E0000 | ~3.3MB | Ramdisk (gzip initramfs) | No |
| 0x840000 | ~116B | Metadata | No |

### Stock Ramdisk Boot Process

The ramdisk (initramfs) embedded in mtd9 is a self-contained early-boot filesystem (~34 MB).
It uses Marvell's `srvd` (Galois service daemon) for ordered service startup, NOT Android's
`init.rc` (which takes over later from the SquashFS rootfs). Key boot stages from `srvd.conf`:

1. Mount tmpfs on `/dev`, `/sys`, `/proc`, `/tmp`, `/etc/tmpfs`
2. `mdev -s` — populate `/dev` with device nodes
3. `sysctl -w kernel.hotplug=/sbin/mdev` — enable hotplug
4. Bring up loopback (`ifconfig lo up`)
5. Mount factory_setting partition (`/bin/mount_part -o no-hack,inband-tags factory_setting`)
6. Read MAC address from `/tmp/factory_setting/MAC_ADDR` or U-Boot env MTD partition via `ethconfig`
7. Start DHCP on `eth0` (USB tethering): `udhcpc -i eth0 -s /etc/udhcpc.script -A 5 -t 8 -b`
8. Start `telnetd` (ramdisk has a telnet daemon for early debug access)
9. Run `/etc/init.d/startup.sh` → launches stock services
10. Mount app partition to `/home/galois` (`mount_part app /home/galois`)
11. Run `/home/galois/run.sh` — stock application launcher

The `inittab` starts `adbd-root` (ADB daemon) as a respawning service, and opens login shells
on the serial console (`/dev/console`) and USB gadget serial (`/dev/ttyGS0`). However, once the
main rootfs `init.rc` takes over, it only activates `"adb"` in the USB composite gadget functions
(ACM is configured with 1 instance via `f_acm/instances` sysfs, but not added to the active
`functions` list). So `/dev/ttyGS0` exists as a device node but has no host-side USB endpoint
unless ACM is explicitly added to the gadget function list.

**MoCA configs** (`clink.conf`, `BinMargin.conf`, `RxLevelMargin.conf`) are present in the
ramdisk — these are Entropic/MaxLinear MoCA (Multimedia over Coax) configs inherited from the
generic Marvell Berlin platform. The Invoke has no coax port; they are unused.

**`SysMonitor.cfg`** points to `10.38.54.91:10001` — an internal Harman/Marvell debug monitoring
server. Not functional on production devices.

## Memory Map

```
Physical Address    Size        Description
────────────────    ────        ───────────
0x00000000          0x01100000  Boot ROM / reserved
0x01100000          0x00F00000  Vendor PHYS_OFFSET region (bootloader, shared mem)
0x02000000          0x20000000  Main RAM (512 MB) — DT memory node
  0x02008000                    Kernel load address (zreladdr)
0x12000000          0x0D600000  SHM region 1 (214 MB) — Marvell inter-processor shared memory
0x1F600000          0x00900000  SHM region 2 (9 MB) — Marvell inter-processor shared memory

Linux sees 248 MB (MemTotal: 253952 kB). The remaining ~223 MB is consumed by the
two SHM regions (214 + 9 MB) reserved by the Galois SHM driver for AMP/fusion IPC.
The 6.1 kernel does not use SHM and can reclaim this memory.

MMIO Regions (from /proc/iomem):
0xF7AB0000          0x200       mmc1 (SDHCI)
0xF7AB0800          0x200       mmc0 (SDHCI, WiFi)
0xF7E81400          0x100       i2c0
0xF7E81500          0x100       i2s
0xF7E81C00          0x100       spi
0xF7ED0000          0x10000     ehci_hcd (USB)
0xF7F00000          0x10000     NFC (NAND flash controller)
0xF7FC2000          0x100       watchdog
0xF7FC9000          0x20        serial (UART0, console)
0xF7901000                      GIC distributor (not in iomem)
0xF7902000                      GIC CPU interface (not in iomem)
```

## Driver Inventory

### Mainline Drivers (should work out of box)

| Driver | Compatible String | Kernel Config | Status |
|--------|-------------------|---------------|--------|
| UART | `snps,dw-apb-uart` | `SERIAL_8250_DW` | In 6.13 DTS |
| I2C | `snps,designware-i2c` | `I2C_DESIGNWARE_PLATFORM` | In 6.13 DTS |
| GPIO | `snps,dw-apb-gpio` | `GPIO_DWAPB` | In 6.13 DTS |
| Watchdog | `snps,dw-wdt` | `DW_WATCHDOG` | In 6.13 DTS |
| Timer | `snps,dw-apb-timer` | `DW_APB_TIMER` | In 6.13 DTS |
| Arch Timer | `arm,armv7-timer` | `ARM_ARCH_TIMER` | In 6.13 DTS |
| GIC | `arm,cortex-a15-gic` | `ARM_GIC` | In 6.13 DTS |
| IRQ Controller | `snps,dw-apb-ictl` | `DW_APB_ICTL` | In 6.13 DTS |
| USB PHY | `marvell,berlin2cd-usb-phy` | `PHY_BERLIN_USB` | In 6.13 DTS |
| USB OTG | `chipidea,usb2` | `USB_CHIPIDEA` | In 6.13 DTS |
| SDHCI | `mrvl,pxav3-mmc` | `MMC_SDHCI_PXAV3` | In 6.13 DTS |
| WiFi | mwifiex (88W8887) | `MWIFIEX_SDIO` | In 6.13 defconfig |
| SquashFS | — | `SQUASHFS_ZLIB` | Config only |

### Drivers Requiring Porting

| Driver | Vendor Source | Effort | Phase |
|--------|-------------|--------|-------|
| **PMIC (88PG868)** | Already in mainline (`drivers/regulator/88pg86x.c`) | Low (config + DT) | 3 |
| **PLL/Clock** | `vendor/kernel/drivers/clk/berlin/pll-berlin2cdp.c` | Medium | 3 |
| **OPP/DVFS** | `vendor/kernel/arch/arm/mach-berlin/opp.c` | Medium | 3 |
| **NAND (NFC)** | `vendor/kernel/arch/arm/mach-berlin/modules/nfc/pxa3xx_nand_debu.c` (NFC = NAND Flash Controller) | High | 2 |
| **Audio (I2S/ASoC)** | `vendor/kernel/sound/soc/berlin/` | Very High | 4 |
| **YAFFS2** | `vendor/kernel/fs/yaffs2/` or staging tree | Medium | 2 |

### Not Needed (Encore handles in userspace)

| Function | Encore Implementation |
|----------|-------------------|
| DSP SPI firmware upload | Direct SPI via `/dev/spidev` or MMIO |
| MCU I2C protocol | Via `/dev/i2c-0` |
| LED ring animation | Via MCU I2C |
| Bluetooth | Raw kernel HCI/L2CAP sockets |

## Clock Strategy

The BG2CDP has no clock driver in mainline Linux. Only BG2, BG2CD, and BG2Q have upstream
clock support.

**Phase 1-2 (Minimal Boot + Core Peripherals):**
Use a 25 MHz fixed oscillator (`refclk`) for all clocks. This works because:
- UART clock is literally the 25 MHz oscillator
- ARM generic timer runs at 25 MHz from the oscillator
- DW APB timers use the oscillator directly
- I2C/GPIO/WDT don't need precise clocks
- CPU stays at whatever frequency the bootloader set (likely 1.0-1.3 GHz)

**Phase 3+ (WiFi, Audio):**
Port vendor PLL driver from `vendor/kernel/drivers/clk/berlin/pll-berlin2cdp.c`:
- `cpupll` at 0xF7920070 — CPU frequency scaling
- `syspll` at 0xF7EA0200 — system bus clocks
- Derived clocks: `cfgclk`, `perifclk`, `nfceccclk`, `sd0clk`
- Audio AVPLL — separate channel-based clock for I2S timing

## CPU Voltage Regulation (PMIC)

### Hardware

The Marvell 88PG868 PMIC sits on I2C bus 0 at address 0x19. It provides two voltage rails:

| Rail | Register | Use | Range |
|------|----------|-----|-------|
| **BUCK1** (BK1_TV) | 0x24 | CPU core voltage (VDD_CPU) | 1.0V–1.35V (DT constrained) |
| **BUCK2** (BK2_TV) | 0x13 | Secondary rail (unused on Invoke) | 1.0V–1.85V |

BUCK1 step sizes: 25mV (below 1.6V), 50mV (above 1.6V). Always-on, boot-on.

### Vendor Kernel (3.8.13) — Full DVFS

The stock kernel has full dynamic voltage and frequency scaling:

**Config:**
- `CONFIG_CPU_FREQ=y` — default governor: `performance` (max frequency)
- `CONFIG_REGULATOR=y` + `CONFIG_REGULATOR_88PG86X=y`
- `CONFIG_GENERIC_CPUFREQ_CPU0=y`

**DVFS chain:**
1. **OTP leakage read** (`mach-berlin/opp.c`): reads silicon leakage from OTP at `0x1010020`
2. **Leakage-to-voltage table** (`berlin2cdp_lv[]`):

   | Leakage (mA) | V_high (>1GHz) | V_low (≤1GHz) |
   |--------------|----------------|----------------|
   | >353 | 1.025V | 0.975V |
   | 305–353 | 1.050V | 1.000V |
   | 257–305 | 1.075V | 1.025V |
   | 234–257 | 1.100V | 1.050V |
   | 193–234 | 1.125V | 1.075V |
   | 0–193 | 1.150V | 1.100V |

3. **OPP table** built at boot: 600, 800, 1000, 1300 MHz
4. **cpufreq-cpu0** driver scales voltage before increasing frequency, after decreasing
5. **Performance governor** (default) → CPU runs at 1300 MHz with V_high

**DT binding (vendor):**
```
compatible = "marvell,pg86x";     /* vendor-specific compat string */
vcpu:BK1_TV@0 { ... };            /* vendor subnode naming */
```

### Linux 6.1 — Current State

**No voltage/frequency control configured:**
- `CONFIG_REGULATOR` is NOT set
- `CONFIG_CPUFREQ` is NOT set
- No BG2CDP clock driver in mainline (only BG2/BG2Q)
- DT `operating-points` defined (1.2V for all frequencies) but inert — no driver uses them
- CPU stays at whatever frequency/voltage the bootloader or vendor kernel set

**Safety analysis:**

| Boot path | Voltage source | Frequency source | Safe? |
|-----------|---------------|------------------|-------|
| **Kexec** (vendor → 6.1) | Vendor kernel set V_high (leakage-optimized) | Vendor kernel set 1300MHz via PLL | **Yes** — optimal |
| **USB boot** (U-Boot → 6.1) | U-Boot's default (likely PG868 power-on default) | U-Boot's PLL config | **Likely** — needs testing |
| **Direct NAND** | N/A | N/A | N/A (encrypted) |

For kexec, the vendor kernel's performance governor sets 1300MHz + appropriate voltage
before kexec executes. The 6.1 kernel doesn't touch either → CPU runs stably.

For USB boot, U-Boot configures the PLL and likely sets a safe voltage. The PG868's
power-on default is undocumented but hardware regulators typically default to mid-range
or maximum voltage for safety.

### Mainline 6.1 PMIC Driver

The 88PG86x driver **already exists in mainline** at `drivers/regulator/88pg86x.c`:
- Uses modern `regmap` API (vs vendor's raw `i2c_smbus`)
- Compatible strings: `marvell,88pg867` or `marvell,88pg868` (different from vendor's `marvell,pg86x`)
- Subnode names: `buck1`, `buck2` (different from vendor's `BK1_TV`, `BK2_TV`)

**To enable (Phase 3+ — NOT needed for initial boot):**

1. Add to `.config`:
   ```
   CONFIG_REGULATOR=y
   CONFIG_REGULATOR_88PG86X=y
   ```

2. Add PMIC node to `berlin2cdp-hk-invoke.dts`:
   ```dts
   &i2c0 {
       pg868: pmic@19 {
           compatible = "marvell,88pg868";
           reg = <0x19>;

           vcpu: buck1 {
               regulator-boot-on;
               regulator-always-on;
               regulator-min-microvolt = <1000000>;
               regulator-max-microvolt = <1350000>;
           };
       };
   };
   ```

3. Add CPU supply reference to `berlin2cdp.dtsi` cpu@0:
   ```dts
   cpu0: cpu@0 {
       cpu0-supply = <&vcpu>;
       ...
   };
   ```

4. For cpufreq, also need:
   - `CONFIG_CPUFREQ=y` + `CONFIG_CPUFREQ_DT=y` (modern equivalent of cpufreq-cpu0)
   - BG2CDP clock driver (port from vendor `pll-berlin2cdp.c`)
   - OPP table with leakage-based voltage (port from vendor `opp.c`, or use fixed 1.2V as conservative fallback)

### Key Files

| File | Purpose |
|------|---------|
| `vendor/kernel/drivers/regulator/88pg86x.c` | Vendor PMIC driver (raw I2C) |
| `vendor/kernel/arch/arm/mach-berlin/opp.c` | Vendor OPP/leakage driver |
| `vendor/kernel/drivers/cpufreq/cpufreq-cpu0.c` | Vendor cpufreq driver |
| `vendor/kernel/drivers/clk/berlin/pll-berlin2cdp.c` | Vendor CPU PLL driver |
| `firmware/.../drivers/regulator/88pg86x.c` | Mainline 6.1 PMIC driver (regmap) |
| `firmware/.../Documentation/.../regulator/88pg86x.txt` | Mainline DT binding |

## Device Tree

### SoC Include (`berlin2cdp.dtsi`)
Ported from vendor 3.8.13 DTS to mainline format. Lives at:
`arch/arm/boot/dts/berlin2cdp.dtsi` (6.1) or
`arch/arm/boot/dts/synaptics/berlin2cdp.dtsi` (6.13)

Key differences from vendor:
- Uses mainline-compatible `compatible` strings (e.g., `snps,dw-apb-uart` not `marvell,berlin-uart`)
- Uses `refclk` fixed oscillator instead of vendor PLL tree
- Omits vendor-specific nodes (AMP, PE, CC, fastlogo, shm)
- No pinctrl driver — pin muxing done by bootloader (static, never changes at runtime)

### Board DTS (`berlin2cdp-hk-invoke.dts`)
Enables the specific peripherals used by the Invoke:
- UART0 (console)
- SDHCI0 (WiFi — 4-bit, non-removable)
- I2C0-2 (MCU, DAC, IO Expander)
- USB0 (gadget serial for debug)
- NAND (disabled during initial testing)

### Known DTS Issues from 6.13 Attempt
1. **No clock tree** — using `refclk` for everything (OK for Phase 1, blocks WiFi)
2. **NAND disabled** — intentionally, to protect stock firmware during testing
3. **No SPI node** — needed for DSP communication
4. **No audio nodes** — I2S, DMA hub, codec all absent
5. **Missing `cpu-ctrl` node** — needed for SMP bringup (not needed for UP boot)
6. **DTS subdirectory**: 6.1 uses `arch/arm/boot/dts/` directly; 6.13+ uses
   `arch/arm/boot/dts/synaptics/`
7. **No PMIC node** — 88PG868 at I2C 0x19 not in DTS, no `cpu0-supply` reference
8. **SMP broken**: Mainline `platsmp.c` looks for `arm,cortex-a9-scu` (BG2/BG2Q are A9),
   but BG2CDP has Cortex-A7 (integrated SCU, no separate IP). The `scu_enable()` path fails
   silently → `cpu_ctrl` never set → `berlin_boot_secondary()` returns -EFAULT. Needs a
   Cortex-A7-specific SMP path or vendor-style `sw_generic1` register approach.
9. **No L2 cache controller node** — vendor calls `l2x0_of_init()` explicitly; 6.1 kernel
   has `CONFIG_CACHE_L2X0=y` but no `arm,pl310-cache` node in DTS for BG2CDP

## Boot Bypass Strategies

The bootloader's RSA signature check must be bypassed to boot a custom kernel.

### Strategy A: kexec Module — BUILT AND TESTED ON DEVICE

Build a loadable kernel module for the stock 3.8.13 kernel that provides kexec functionality.
**Status: Module loads, /dev/kexec created, kexec-tools communicates with it.**

**How it works:**
1. `kexec-mod.ko` force-defines `CONFIG_KEXEC=1` and compiles the vendor's own
   `kernel/kexec.c` + `arch/arm/kernel/machine_kexec.c` + `relocate_kernel.S` as a module
2. Uses `kallsyms_lookup_name()` to resolve 4 unexported symbols at init:
   `machine_shutdown`, `soft_restart`, `memblock_is_region_memory`, `kernel_restart_prepare`
3. Creates `/dev/kexec` character device (ioctl interface)
4. Patched `kexec-tools 2.0.15` calls `/dev/kexec` ioctls directly (static binary, no LD_PRELOAD)

**Components (all built and verified on device):**
| File | Size | Description |
|------|------|-------------|
| `tools/kexec-module/kernel/kexec-mod.ko` | 19,844 B | Kernel module (vermagic: 3.8.13-yocto-standard) |
| `tools/kexec-module/user/redir.so` | 7,500 B | LD_PRELOAD shim (not needed with patched kexec) |
| `tools/kexec-tools/.../build/sbin/kexec` | 480,824 B | Static ARM binary, patched for /dev/kexec |

**Build gotchas:**
- GCC 13 + kernel 3.8.13: need `compiler-gcc{5..13}.h` stubs (`#include <linux/compiler-gcc4.h>`)
- gnueabihf `cc-option` fails for `-march=armv7-a` (hard-float conflict), fix with
  `CFLAGS_MODULE='-march=armv7-a -msoft-float -mfloat-abi=soft -fno-pic -fno-pie'`
- kexec-tools 2.0.15 + GCC 10+: add `CFLAGS='-fcommon'` (my_debug multiple definition)
- Vendor kernel tree has `CONFIG_LOCALVERSION=""` but running kernel is `-yocto-standard` —
  fix `include/generated/utsrelease.h` before building

**SMP handling:** `machine_shutdown()` calls `smp_send_stop()` → IPI to CPU1 → CPU1 enters
infinite `cpu_relax()` loop with IRQs disabled. No HOTPLUG_CPU needed.

**Permanent boot flow:**
1. Stock encrypted kernel boots from NAND (3.8.13-yocto-standard)
2. Our rootfs runs `mount_partition.sh` → loads Encore
3. Encore (or boot script) loads `kexec-mod.ko` from `/lsync/kexec/`
4. `kexec -l /lsync/kexec/zImage-dtb` loads the new kernel via /dev/kexec ioctl
5. `kexec -e` executes: `machine_shutdown()` → `machine_kexec()` → custom 6.1 kernel boots

**Critical implementation details:** See `docs/kexec-method.md` for the full technical
reference. Key insight: identity mapping must use Normal WB-WA memory type (section
descriptor 0x40E, not 0x402) because Cortex-A7 cannot fetch instructions from Strongly
Ordered memory under TEX remap. Our custom `machine_kexec()` uses `setup_mm_for_reboot()`
to switch to idmap_pgd, then adds a Normal-memory-type identity mapping for the reboot
code buffer. `relocate_kernel.S` handles local-only cache flush (ICIALLU instead of
ICIALLUIS to avoid the SMP hang).

**Prior art:** [Steam Link kexec](https://github.com/mill1000/steamlink-kexec) on BG2CD
(same SoC family). Based on [amonakov/kexec-module](https://github.com/amonakov/kexec-module).

### Strategy B: USB Boot with Custom 81_IMAGE — CONFIRMED VIABLE

**The USB boot path loads PLAINTEXT kernels.** Analysis of the OTA2 package reveals:

The stock 81_IMAGE is a **standard U-Boot legacy image** (`0x27051956` magic) containing an
unencrypted zImage. The USB boot flow is:

1. Boot ROM loads encrypted `bootloader.img` + `sysinit.img` + `bcm_erom.bin.usb`
   (all Marvell-signed, using the BCM security coprocessor for decryption)
2. The decrypted bootloader provides a U-Boot shell
3. `79_IMAGE` script runs: `usbload 0x81 0x0C400000` loads the **plaintext** kernel to RAM
4. `set bootargs ...` configures kernel command line
5. `bootm 0x0C400000` boots the kernel using standard U-Boot image header parsing

The 81_IMAGE on NAND (mtd9) is encrypted, but the 81_IMAGE loaded via USB is plaintext.
The bootloader handles two separate code paths: NAND boot (decrypt+verify) vs USB boot
(direct load+execute).

**To boot a custom kernel:**
```bash
# Create U-Boot image from our zImage-dtb
mkimage -A arm -O linux -T kernel -C none \
  -a 0x02008000 -e 0x02008000 \
  -n "Linux-6.1-hkinvoke" -d zImage-dtb 81_IMAGE

# Create 79_IMAGE script
cat > 79_IMAGE << 'EOF'
usbload 0x81 0x0c400000
set bootargs console=ttyS0,115200 earlyprintk loglevel=8 panic=5
bootm 0x0c400000
EOF
```

**Stock 81_IMAGE details:**
- Size: 3,288,888 bytes (3.14 MB)
- Format: U-Boot legacy image (magic `0x27051956`)
- Load address: `0x02008000`
- Entry point: `0x02008000`
- Compression: none
- Name: "Linux-3.8.13-mrvl"
- zImage at offset 0x40 (after 64-byte U-Boot header)

### Strategy C: Direct NAND Write — RULED OUT

The kernel on NAND is AES-encrypted. Writing a plaintext zImage to mtd9 would produce
garbage when the bootloader tries to decrypt it. No viable path here.

## Phased Plan

### Phase 0: Documentation & Setup ✓
- [x] Create this document
- [x] Extract `linux-6.1-latest.tar.xz` in WSL
- [x] Create `scripts/build/build_kernel.sh`
- [x] Document boot security findings
- [x] Document driver inventory

### Phase 1: Analyze Boot Chain
- [x] Dump and analyze stock bootimgs (mtd9) header format
- [ ] Test USB boot signature behavior
- [x] Check stock kernel module loading (`CONFIG_MODULES`, `/dev/mem`)
- [x] Dump `/proc/iomem`, `/proc/mtd` from running device
- **Deliverable:** Know whether USB boot bypasses signature check, and whether
  module loading works for the kexec approach

#### Phase 1 Findings

**Kernel image is AES-ENCRYPTED** (not just signed):
- The bootimgs partition (mtd9, 10MB) contains 3 sub-images:
  - **Kernel** (0x020000, ~4.9MB): `01 00 00 00` flag + 3x1024-byte signing headers + encrypted payload.
    Entropy = 8.00 bits/byte (maximum — indistinguishable from random = AES encryption)
  - **Ramdisk** (0x4E0000, ~3.3MB): Plain gzip (1f 8b magic). NOT encrypted.
  - **Metadata** (0x840000, ~116 bytes): Small data block, not a DTB.
- bootimgs header at offset 0x2C stores load address `0x02008000`
- Kernel data size: 4,940,006 bytes (0x4B60E6)
- The signing header has nested `01 00 00 00` markers at offsets 0x0, 0x400, 0x800
  (each 1024 bytes, likely: outer envelope → signing key → encrypted payload header)

**Module loading works:**
- `CONFIG_MODULES=y`, `CONFIG_MODULE_SIG` is NOT set, `CONFIG_MODULE_FORCE_LOAD=y`
- `/dev/mem` exists and is accessible
- Vermagic: `3.8.13-yocto-standard SMP preempt mod_unload ARMv7`
- Vendor kernel tree (`vendor/kernel/`) has Module.symvers, .config, auto.conf — ready for module builds
- GCC 13.3 in WSL (vs stock GCC 5.3) — acceptable, vermagic only checks kernel version

**kexec module CONFIRMED WORKING :**
- Module loads without signature checking (CONFIG_MODULE_SIG not set)
- All 4 kallsyms lookups succeed: machine_shutdown, soft_restart, memblock_is_region_memory, kernel_restart_prepare
- `/dev/kexec` created (major 250), kexec-tools communicates via ioctl
- `kexec -u` (unload) returns exit 0 — full round-trip confirmed
- **USB boot (Strategy B) also viable** — for development iteration

### Phase 2: Build Minimal 6.1 Kernel
- Build `zImage-dtb`
- **Deliverable:** `zImage-dtb` ready for boot testing

### Phase 3: First Boot Attempt via USB Boot
**Using the USB boot path (Strategy B) — plaintext kernel loading confirmed.**

Steps:
1. Build the 6.1 zImage-dtb (Phase 2)
2. Wrap in U-Boot legacy image: `mkimage -A arm -O linux -T kernel -C none -a 0x02008000 -e 0x02008000 -n "Linux-6.1" -d zImage-dtb 81_IMAGE`
3. Create custom 79_IMAGE with boot commands
4. Enter USB boot mode (reset + mic-mute x4)
5. Run usb_boot.exe — it loads the stock encrypted bootloader, then our plaintext kernel
6. Monitor serial console (ttyS0 @ 115200) for kernel output

No kexec module needed! The USB boot path provides direct, unencrypted kernel loading.

- [ ] Package zImage-dtb as 81_IMAGE with mkimage
- [ ] Create 79_IMAGE with custom bootargs
- [ ] Test USB boot with custom kernel
- [ ] Monitor serial console
- [ ] Iterate on DTS/config based on boot output
- **Deliverable:** Serial console output from new kernel (even partial/panic is success)

### Phase 4: Driver Porting (incremental)

| Priority | Driver | Approach |
|----------|--------|----------|
| P0 | UART, I2C, GPIO, WDT | Mainline drivers, should work |
| P1 | NAND (MTD) | Try mainline `MTD_NAND_MARVELL`, fall back to vendor port |
| P1 | SquashFS + YAFFS2 | Config + possible backport |
| P2 | PLL/Clock | Port vendor `pll-berlin2cdp.c` to 6.1 clock framework |
| P2 | SDHCI + WiFi (mwifiex) | Needs clock tree working |
| P3 | Audio (I2S/ASoC) | Major vendor driver port (~15K lines) |
| P3 | USB | Mainline chipidea, needs PHY |
| P4 | Bluetooth | Encore uses raw kernel sockets — may not need kernel driver |

### Phase 5: Integration
- [ ] Build SquashFS rootfs with new kernel modules
- [ ] Package as flashable firmware image
- [ ] Test all Encore subsystems against new kernel
- [ ] Enable SMP with HOTPLUG_CPU
- [ ] Performance comparison vs 3.8.13

## Key Files

The `vendor/`, `firmware/kernel_test/`, and `tools/kexec-*` paths below are local working
trees, not part of this repository. Obtain the vendor kernel from Harman's GPL source
release (linked under External References) and the 6.1 tree from kernel.org; the kexec
module sources are not yet published.

| File | Purpose |
|------|---------|
| `docs/kernel-porting.md` | This document |
| `scripts/build/build_kernel.sh` | Kernel build script |
| `scripts/build/build_kexec.sh` | kexec module + tools build script |
| `scripts/kexec/deploy_kexec.sh` | Deploy kexec to device |
| `tools/kexec-module/kernel/` | kexec loadable kernel module source |
| `tools/kexec-module/user/` | redir.so LD_PRELOAD shim source |
| `tools/kexec-tools/` | kexec-tools 2.0.15 (patched for /dev/kexec) |
| `firmware/kernel_test/linux-6.1.*/` | Kernel source tree |
| `vendor/kernel/` | Stock 3.8.13 kernel tree (module build host) |
| `vendor/kernel/.config` | Stock kernel config |
| `vendor/kernel/arch/arm/boot/dts/berlin2cdp.dtsi` | Vendor SoC device tree |
| `vendor/kernel/drivers/clk/berlin/pll-berlin2cdp.c` | Vendor PLL driver |
| `vendor/kernel/arch/arm/mach-berlin/modules/nfc/` | Vendor NAND driver |
| `vendor/kernel/sound/soc/berlin/` | Vendor audio driver (9 files) |

## External References

- Chromecast bootloader source (GPL): https://pastebin.com/3c1BUieq
- Chromecast tools (crypto header analysis): https://github.com/tchebb/chromecast-tools
- Google Home Mini exploit (same SoC): https://courk.cc/running-custom-code-google-home-mini-part1
- Harman Kardon Invoke kernel GPL source: https://archive.org/details/invoke-kernel
- Steam Link mainline kernel (BG2CD): https://heap.ovh/getting-linux-on-valve-steam-link.html
- kexec-module: https://github.com/amonakov/kexec-module
- kexec-mod (ARM64 variant): https://github.com/fabianishere/kexec-mod
- HKHacking community: https://github.com/coggy9/HKHacking
