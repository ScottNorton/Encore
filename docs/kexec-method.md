# kexec Boot Method for Harman Kardon Invoke

> **Status:** research log. This documents the working kexec path and the debugging that
> led to it, largely in chronological order; some "open questions" near the end have since
> been answered by later work (see [kernel-porting.md](kernel-porting.md) for the current
> state of the Linux 6.1 port). The kexec module and patched kexec-tools sources referenced
> under `tools/` are not yet included in the public repository.

## What This Is

A method to boot a custom Linux kernel on the Harman Kardon Invoke without modifying the
encrypted NAND bootchain. The stock 3.8.13 kernel boots normally from encrypted
NAND, then a loadable kernel module provides the `kexec` system call, which
overwrites the running kernel in RAM and jumps to a new one.

This is a well-established technique. The Valve Steam Link community has used it
since ~2018 to run mainline Linux on Marvell Berlin BG2CD hardware — the same SoC
family as the Harman Kardon Invoke's BG2CDP.

## Prior Art and Sources

### Primary: amonakov/kexec-module
- **Repository**: https://github.com/amonakov/kexec-module
- **What it is**: Light scaffolding for building Linux `kexec` as a loadable kernel
  module. Compiles the kernel's own `kexec.c` and `machine_kexec.c` as an out-of-tree
  module by force-defining `CONFIG_KEXEC=1`.
- **Target**: Linux 3.8 on x86 and ARM
- **Key innovation**: Uses `kallsyms_lookup_name()` to resolve unexported kernel symbols
  (`machine_shutdown`, `soft_restart`, etc.) at module load time. Creates `/dev/kexec`
  character device with ioctl interface.
- **Two components**:
  - `kernel/` → `kexec-mod.ko` (the loadable module)
  - `user/` → `redir.so` (LD_PRELOAD shim that redirects kexec-tools syscalls to
    `/dev/kexec` ioctls — allows unmodified kexec-tools to work)
- **License**: GPL v2

### Secondary: mill1000/steamlink-kexec
- **Repository**: https://github.com/mill1000/steamlink-kexec
- **What it is**: Complete instructions and tools for booting alternate kernels on
  Valve Steam Link hardware using kexec.
- **Hardware**: Marvell Armada 1500 (BG2CD) — same SoC family as our BG2CDP
- **Stock kernel**: 3.8.13-mrvl (same base version as our 3.8.13-yocto-standard)
- **Custom kernel**: 3.8.14-mrvl (later versions 5.4/5.10 via heap.ovh)
- **Key learnings applied**:
  - Module must match exact vermagic string of running kernel
  - kexec-tools 2.0.15 works; 2.0.20 does NOT (confirmed by heap.ovh)
  - DTB must be passed explicitly via `--dtb` flag
  - Watchdog must be handled (will reboot device if not pet during kexec transition)
  - `--command-line` sets the new kernel's boot arguments

### Reference: heap.ovh Steam Link article
- **URL**: https://heap.ovh/getting-linux-on-valve-steam-link.html
- **What it is**: Detailed writeup of running mainline Linux 5.4/5.10 on Steam Link
  via kexec from the stock 3.8.13 kernel.
- **Working hardware on BG2CD after kexec**: Ethernet, USB, WLAN, Bluetooth, I2C,
  temperature sensors, UART
- **Not working**: NAND, DMA, video/audio output, suspend/resume
- **Exact kexec command**:
  ```
  kexec -l /boot/zImage --initrd /boot/initramfs.img \
    --dtb /boot/berlin2cd-valve-steamlink.dtb \
    --command-line "root=/dev/sda1 rootwait rw console=ttyS0,115200n8"
  ```

## How It Works (Technical Detail)

### Architecture

```
┌─────────────────────────────────────────────────────┐
│  NAND (encrypted)                                   │
│  ┌───────────┐  ┌──────────┐  ┌──────────────────┐ │
│  │ Bootloader │→│ Stock    │→│ Our SquashFS     │ │
│  │ (AES+RSA) │  │ 3.8.13   │  │ rootfs           │ │
│  └───────────┘  │ (AES)    │  │                  │ │
│                 └──────────┘  └──────────────────┘ │
└─────────────────────────────────────────────────────┘
                       │
                       ▼
              Stock kernel boots
                       │
                       ▼
          mount_partition.sh runs
                       │
                       ▼
       insmod /lsync/kexec/kexec-mod.ko
           → /dev/kexec created
                       │
                       ▼
       kexec -l /lsync/kexec/zImage-dtb
           → new kernel loaded into RAM
                       │
                       ▼
              kexec -e
                       │
    ┌──────────────────┼──────────────────┐
    │                  ▼                  │
    │   machine_shutdown()                │
    │     → smp_send_stop()               │
    │       → CPU1 enters WFI loop        │
    │                  ▼                  │
    │   kernel_restart_prepare()          │
    │     → notifier chain shutdown       │
    │                  ▼                  │
    │   machine_kexec()                   │
    │     → disable IRQs/FIQs            │
    │     → setup_mm_for_reboot()        │
    │       → TTBR0 = idmap_pgd          │
    │     → idmap_add_current()          │
    │       → Normal WB-WA descriptor    │
    │     → jump to phys addr             │
    │                  ▼                  │
    │   relocate_new_kernel (asm)         │
    │     → pet WDT (direct MMIO)        │
    │     → flush caches (local only)     │
    │     → disable MMU                   │
    │     → copy segments to final addr   │
    │     → r0=0, r1=mach_type           │
    │     → r2=DTB_addr                  │
    │     → pc=kernel_entry (0x02008000) │
    │                  ▼                  │
    │   *** 3.8.13 IS GONE ***           │
    │   New 6.1 kernel decompresses      │
    │   and boots from scratch            │
    └─────────────────────────────────────┘
```

### Module Internals

The module (`kexec-mod.ko`) is built from 4 object files:

| Object | Source | Purpose |
|--------|--------|---------|
| `kexec-drv.o` | Our `kexec-drv.c` | Driver: `/dev/kexec`, ioctl handler, kallsyms resolution |
| `kexec.o` | Vendor's `kernel/kexec.c` (wrapped) | Core kexec syscall implementation |
| `machine_kexec.o` | Our custom `machine_kexec.c` | ARM kexec: identity mapping, TTBR0 switch, jump to phys addr |
| `relocate_kernel.o` | Our custom `relocate_kernel.S` | Assembly trampoline: WDT pet, cache flush, MMU off, relocate, jump |

The trick: `kexec-mod.h` force-defines `CONFIG_KEXEC=1` and is injected via
`-include` into every translation unit. This makes the vendor's `#ifdef CONFIG_KEXEC`
code compile even though the running kernel was built without it.

### Patched kexec-tools

Standard kexec-tools uses `syscall(SYS_kexec_load, ...)` and `reboot(LINUX_REBOOT_CMD_KEXEC)`.
Since the stock kernel has no kexec syscall (CONFIG_KEXEC=n), these would fail.

The original amonakov approach uses `redir.so` (LD_PRELOAD) to intercept these calls and
redirect to `/dev/kexec` ioctls. But our kexec binary is statically linked (the device has
old glibc), so LD_PRELOAD doesn't work.

**Our fix**: Patched `kexec-syscall.h` directly — `kexec_load()` tries
`open("/dev/kexec")` first, falls back to real syscall. Similarly, `my_exec()` in
`kexec.c` tries the `/dev/kexec` ioctl before `reboot()`. No LD_PRELOAD needed.

## Symbol Resolution

Six symbols resolved via `kallsyms_lookup_name()` at module init:

| Symbol | Address | Source | Purpose |
|--------|---------|--------|---------|
| `machine_shutdown` | `0xc000e9c0` | kallsyms | Stops CPU1 via `smp_send_stop()` IPI |
| `soft_restart` | `0xc000e9e8` | kallsyms | Original soft_restart (not used — hangs on flush_cache_all) |
| `memblock_is_region_memory` | `0xc04a70d4` | kallsyms | Validates kexec segments are in physical RAM |
| `kernel_restart_prepare` | `0xc0048a94` | kallsyms | Runs reboot notifier chain |
| `setup_mm_for_reboot` | `0xc0019478` | kallsyms | Switches TTBR0 to idmap_pgd |
| `idmap_pgd` | `0xc062ac30` | **BSS (static)** | Identity-mapped page table (not in kallsyms) |

### How idmap_pgd is Accessed

`idmap_pgd` is a static BSS variable in `arch/arm/mm/idmap.c`. It's NOT exported
and NOT visible through `kallsyms_lookup_name()`.

**Solution**: Instead of hardcoding the address, we call `setup_mm_for_reboot()` to
switch TTBR0 to idmap_pgd, then read TTBR0 via `mrc p15, 0, rN, c2, c0, 0` to
discover the physical address of idmap_pgd. We convert this to a kernel VA via
`phys_to_virt()` and write our section descriptor directly.

The `idmap_pgd` page table has:
- **Kernel portion** (>= 0xC0000000): copied from `swapper_pg_dir` (full kernel VA space)
- **User portion** (< 0xC0000000): identity mappings for `.idmap.text` section only

We add our own 1MB section identity mapping to the user portion for the
reboot_code_buffer's physical address.

## Identity Mapping: The Hard Problem

### Why Identity Mapping is Needed

When kexec jumps to `relocate_new_kernel`, the code needs to:
1. Run at a PHYSICAL address (so it survives MMU disable)
2. Flush caches, disable MMU, relocate kernel segments, jump to new kernel

For step 1, the CPU is in virtual memory mode — every instruction fetch goes through
the page table. We need a mapping where VA == PA (identity mapping) so that when we
disable the MMU, the program counter (which holds a physical address) still points to
valid memory.

### What Failed (and Why)

Multiple iterations of the identity mapping code crashed the device. Root cause analysis
via vendor kernel source comparison revealed several critical issues. The most important
was the TEX remap / memory type bug — this was the **actual root cause** of every crash.

#### ROOT CAUSE: Strongly Ordered Memory Type (TEX Remap)

**Bug**: The section descriptor used `TEX=000, C=0, B=0` which under TEX remap
(SCTLR.TRE=1) maps to **Strongly Ordered memory** via PRRR index 0. The Cortex-A7
**cannot fetch instructions from Strongly Ordered memory** — any attempt causes a
Prefetch Abort, regardless of the XN bit setting.

**TEX remap table** (vendor kernel: PRRR=`0xff0a81a8`, NMRR=`0x40e040e0`):

| Index n | TEX[0],C,B | PRRR TR | Memory Type | Inst Fetch? |
|---------|-----------|---------|-------------|-------------|
| 0 | 0,0,0 | 00 | Strongly Ordered | **NO** |
| 1 | 0,0,1 | 10 | Normal, Non-cacheable | Yes |
| 2 | 0,1,0 | 10 | Normal, Write-Through | Yes |
| 3 | 0,1,1 | 10 | Normal, WB-WA | **Yes** |
| 4 | 1,0,0 | 01 | Device | **NO** |
| 7 | 1,1,1 | 10 | Normal, WB-WA | Yes |

**Old descriptor (BROKEN)**: `phys | 0x402` → TEX=000, C=0, B=0 → index 0 → Strongly Ordered

**Fixed descriptor**: `phys | 0x40E` → TEX=000, C=1, B=1 → index 3 → Normal WB-WA

**Why the vendor kernel's `identity_mapping_add()` uses 0x402 and works**:
The stock `soft_restart()` sequence calls `cpu_proc_fin()` which disables I-cache and
D-cache in SCTLR *before* jumping to `cpu_v7_reset` at the identity-mapped physical
address. With caches disabled, the Cortex-A7 apparently allows instruction fetch from
Strongly Ordered memory (only 4 instructions before MMU is disabled). Our kexec module
skips `cpu_proc_fin()` to avoid the ICIALLUIS hang, so the caches are still enabled
when we jump — requiring Normal memory type for the identity mapping.

#### Other Fixes Applied

**Scheduler TTBR0 swapping**: The scheduler swaps TTBR0 on every context switch. Any
kernel code (printk, file I/O) between writing the identity mapping and jumping could
trigger rescheduling, losing our mapping. **Fix**: Use `setup_mm_for_reboot()` to switch
to idmap_pgd, then add mapping to the now-active table with IRQs disabled.

**idmap_pgd not in kallsyms**: `idmap_pgd` is a static BSS variable in `idmap.c`, not
visible via `kallsyms_lookup_name()`. **Fix**: Call `setup_mm_for_reboot()` to switch
TTBR0 to idmap_pgd, then read TTBR0 to discover its physical address. No hardcoded
addresses needed.

**DACR not set to Manager**: The identity mapping section uses Domain 0. If DACR has
Domain 0 set to "Client" (permission-checked), the access fails. **Fix**: Set DACR to
`0xFFFFFFFF` (Manager mode for all 16 domains) after switching to idmap_pgd.

### What Works Now

The corrected sequence:

```
1. Copy relocate_new_kernel to control page buffer
   - flush_icache_range() to ensure coherency

2. Set WDT timeout to maximum (via pre-mapped ioremap'd VA)

3. Write CP15 diagnostics to /lsync/kexec/diag.txt (IRQs still enabled)

4. local_irq_disable() + local_fiq_disable()

5. Pet WDT one more time (ioremap'd addresses still work)

6. setup_mm_for_reboot() — kernel's own function switches TTBR0 to idmap_pgd
   (Handles CONTEXTIDR, TTBR0 with TTB flags, TLB flush internally)

7. idmap_add_current() — add identity mapping to the NOW-ACTIVE idmap_pgd:
   a. Read TTBR0 to discover idmap_pgd physical address
   b. Convert to kernel VA via phys_to_virt()
   c. Write section descriptor with NORMAL WB-WA memory type:
      phys_sect | 0x40E  (PMD_TYPE_SECT | AP_WRITE | CACHEABLE | BUFFERABLE)
      TEX=000, C=1, B=1 → PRRR index 3 → Normal Write-Back Write-Allocate
   d. Clean D-cache line (DCCMVAC) + DSB
   e. Set DACR = 0xFFFFFFFF (manager-all)
   f. TLBIALL + ICIALLU + BPIALL
   g. DSB + ISB

8. Jump to reboot_code_buffer at PHYSICAL address
   (identity mapping makes VA == PA, Normal memory allows instruction fetch)

9. relocate_kernel.S takes over:
   - Pet WDT via direct MMIO (0xF7FC200C)
   - Flush D-cache by set/way (local only — no ICIALLUIS)
   - Invalidate I-cache (ICIALLU — local only)
   - Disable MMU (safe because VA == PA)
   - Relocate kernel segments to final addresses
   - r0=0, r1=mach_type, r2=DTB_addr
   - Jump to new kernel entry
```

### Progressive Dry-Run Testing

The module supports a `kexec_dry_run` parameter for non-destructive testing:

```bash
# Level 1: Test TTBR0 switch to idmap_pgd and back
insmod kexec-mod.ko kexec_dry_run=1

# Level 2: + read from physical address through identity mapping
insmod kexec-mod.ko kexec_dry_run=2

# Level 3: + execute NOP+BX_LR at physical address
insmod kexec-mod.ko kexec_dry_run=3

# Then trigger:
kexec -l /lsync/kexec/zImage --dtb=... --command-line=...
kexec -e
# Device should NOT reboot — dry-run returns after test
# Check dmesg for "DRY RUN level N — PASSED" or "FAILED"
```

Each level builds on the previous:
- **Level 1**: Verifies the MM switch itself doesn't crash (TTBR0 + CONTEXTIDR + DACR)
- **Level 2**: Verifies the identity mapping works for data access (read at phys addr)
- **Level 3**: Verifies code execution at physical address (NOP + BX_LR returns)

If level N crashes but level N-1 passes, the problem is isolated to that specific step.

### Why soft_restart() Can't Be Used

The stock `soft_restart()` calls `flush_cache_all()` which uses `ICIALLUIS` — an
"Inner Shareable" I-cache invalidate that broadcasts to all CPUs in the coherency
domain. After `smp_send_stop()` has sent CPU1 into a WFI loop, this broadcast
hangs because CPU1 never acknowledges it.

Our `relocate_kernel.S` uses `ICIALLU` (local I-cache invalidate only) and flushes
D-cache by iterating set/way on the local CPU only. No broadcast operations.

### Vendor soft_restart Sequence (for reference)

The vendor kernel's `soft_restart()` in `arch/arm/kernel/process.c` does:

1. `call_with_stack(__soft_restart, addr, soft_restart_stack + 16)` — switch to
   a static 128-byte stack (avoids corrupting the process stack)
2. `setup_mm_for_reboot()` — switch TTBR0 to idmap_pgd
3. `flush_cache_all()` — first cache flush (**hangs on BG2CDP after smp_send_stop**)
4. `cpu_proc_fin()` → `cpu_v7_proc_fin()` — disable I/D caches, branch prediction
5. `flush_cache_all()` — second flush (caches now off, reads from DRAM)
6. `outer_disable()` — no-op on BG2CDP (no external L2)
7. `cpu_reset(addr)` → `cpu_v7_reset()` — disable MMU, jump to physical address

We skip steps 3-6 entirely. `relocate_kernel.S` handles cache flush with local-only
operations, disables the MMU itself, and jumps directly.

## Build Instructions

### Prerequisites (WSL Ubuntu)
```bash
sudo apt install gcc-arm-linux-gnueabihf make bc flex bison
```

### Build everything
```bash
# From WSL, in the repo root:
bash scripts/build/build_kexec.sh
```

This builds:
1. `kexec-mod.ko` against `vendor/kernel/` (~20 KB)
2. `redir.so` for ARM (7.5 KB, not needed but included)
3. `kexec` static ARM binary from kexec-tools 2.0.15 (481 KB)

### Build individually
```bash
bash scripts/build/build_kexec.sh module   # kexec-mod.ko only
bash scripts/build/build_kexec.sh redir    # redir.so only
bash scripts/build/build_kexec.sh tools    # kexec-tools only
```

### Build gotchas

| Problem | Cause | Fix |
|---------|-------|-----|
| `compiler-gcc13.h: No such file` | Kernel 3.8 only has gcc3/gcc4 headers | Created `compiler-gcc{5..13}.h` stubs in vendor tree |
| `dmb/ldrex not supported in ARM mode` | gnueabihf `cc-option` rejects `-march=armv7-a` | `CFLAGS_MODULE='-march=armv7-a -msoft-float -mfloat-abi=soft'` |
| `Unknown symbol _GLOBAL_OFFSET_TABLE_` | GCC 13 generates PIC by default | Add `-fno-pic -fno-pie` to CFLAGS_MODULE |
| `version magic mismatch` | Vendor tree has `LOCALVERSION=""`, device has `-yocto-standard` | Fixed `utsrelease.h`, `autoconf.h`, `auto.conf` |
| `multiple definition of my_debug` | GCC 10+ defaults to `-fno-common` | `CFLAGS='-fcommon'` for kexec-tools |
| `GLIBC_2.34 not found` | redir.so links against host glibc | Patched kexec-tools directly, no LD_PRELOAD needed |
| Literal pool out of copy range | `ldr r4, =0x3ff` places data after code | Replaced with `movw r4, #0x3ff` in relocate_kernel.S |
| Stale .o files | `build_kexec.sh` doesn't `make clean` | Always run `make clean` in `tools/kexec-module/kernel/` before rebuilding after source changes |

## Deployment

### Upload to device
```bash
# From WSL (or just use scripts/kexec/deploy_kexec.sh which handles this):
DEVICE="${ENCORE_DEVICE_IP:-192.168.43.1}"  # AP default; set .encore-env for LAN IP
SSH="sshpass -p ridiculous ssh -o HostKeyAlgorithms=+ssh-rsa \
  -o PubkeyAcceptedKeyTypes=+ssh-rsa -o StrictHostKeyChecking=no root@$DEVICE"

$SSH "mkdir -p /lsync/kexec"
cat tools/kexec-module/kernel/kexec-mod.ko | $SSH "cat > /lsync/kexec/kexec-mod.ko"
cat tools/kexec-module/user/redir.so       | $SSH "cat > /lsync/kexec/redir.so"
cat tools/kexec-tools/kexec-tools-2.0.15/build/sbin/kexec | \
  $SSH "cat > /lsync/kexec/kexec && chmod +x /lsync/kexec/kexec"
```

Or use the deploy script:
```bash
bash scripts/kexec/deploy_kexec.sh 192.168.43.1
```

### Load module
```bash
# On device:
insmod /lsync/kexec/kexec-mod.ko
# Expected dmesg:
#   kexec-mod: setup_mm_for_reboot=c0019478
#   kexec-mod: all symbols resolved
#   kexec-mod: WDT base mapped at XXXXXXXX
#   kexec-mod: /dev/kexec created (major 250)
```

### Load and execute a kernel
```bash
# On device:
/lsync/kexec/kexec -l /lsync/kexec/zImage \
  --dtb=/lsync/kexec/berlin2cdp-hk-invoke.dtb \
  --command-line="console=ttyS0,115200 earlyprintk loglevel=8"
/lsync/kexec/kexec -e
# Device reboots into new kernel
```

## Confirmed Working Steps

### Module Load
Module loads, all kallsyms resolve (`setup_mm_for_reboot` at 0xc0019478), WDT CRR
mapped at 0xF7FC2000, `/dev/kexec` created.

### Identity Mapping Test — Level 1 PASSED (2026-02-22)
TTBR0 switch to idmap_pgd and back works perfectly:
- `setup_mm_for_reboot()` switches TTBR0 from `0x1a87c06a` to `0x1e85006a`
- `idmap_pgd` physical = `0x1e850000`, virtual = `0xdd750000`
- SCTLR=`0x10c5387d` (TRE=1), PRRR=`0xff0a81a8`, NMRR=`0x40e040e0`
- Restore back to original TTBR0 — **non-destructive, device fully functional after**
- DACR confirmed = `0x00000015` (not 0x55555555 as expected)

### kexec Load
kexec-tools loads kernel image (2 segments: zImage + DTB) via `/dev/kexec` ioctl.
Load and unload cycles work correctly.

Debug output from `kexec -l -d`:
```
MEMORY RANGES
0000000002000000-000000001edfffff (0)
zImage header: 0x016f2818 0x00000000 0x0036a9e0
zImage size 0x36a9e0, file size 0x36d073
```

Segment placement by kexec-tools:
- **ATAGS** at `base + 0x1000` = `0x02001000`
- **Kernel** at `base + 0x8000` = `0x02008000`
- **Initrd** at `kernel_base + ALIGN(kernel_len * 5, pagesize)` (5x safety margin for decompression)

ATAGS offset is consistent between kexec-tools (hardcoded 0x1000) and the module
fallback (vendor header `KEXEC_ARM_ATAGS_OFFSET = 0x1000` in `asm/kexec.h`):
`image->start - 0x8000 + 0x1000 = 0x02008000 - 0x8000 + 0x1000 = 0x02001000`

### relocate_new_kernel Execution
Custom `relocate_kernel.S` includes a WDT pet loop at the start (runs with MMU/caches
off, direct physical MMIO). With 200 iterations, the device stayed alive for 30+ minutes
without rebooting — conclusive proof that the trampoline code reached execution.
Reducing to 3 iterations allowed the relocation to complete and jump to the new kernel.

### Initramfs
A busybox-based initramfs for evidence collection:
- Static ARM BusyBox 1.36.1 (1,496,892 bytes) with full applet set including `devmem`,
  `i2c*`, `nandwrite`, `dd`, `nanddump` — but NOT `flash_eraseall`
- `/init` script: mounts proc/sys/dev, collects evidence, writes to NAND, pets WDT
- Source: `firmware/initramfs_tiny/`
- Built archive: `firmware/initramfs_kexec.cpio.gz`
- Rebuild script: `scripts/rebuild_kexec_initramfs.sh`

## Kexec Boot Testing (2026-02-22)

### Test Setup
- **Kernel**: Stock vendor 3.8.13 zImage with appended DTB (`zImage-vendor`, 3,526,960 bytes)
  - This is the SAME kernel the device already runs — eliminates driver/DTS unknowns
  - Has `CONFIG_BLK_DEV_INITRD=y`, `CONFIG_RD_GZIP=y`, `CONFIG_ARM_APPENDED_DTB=y`,
    `CONFIG_ARM_ATAG_DTB_COMPAT=y`, `CONFIG_ARM_ATAG_DTB_COMPAT_CMDLINE_FROM_BOOTLOADER=y`
  - Does NOT have: `CONFIG_DEVTMPFS` (must use manual mknod), `CONFIG_STRICT_DEVMEM`
- **Initramfs**: BusyBox 1.36.1 (ARM static, 1.4 MB), loaded separately via `--initrd`
- **Goal**: Prove the kexec'd kernel boots and initramfs runs

### Critical Discovery: --atags Flag Required

Without `--atags`, kexec-tools 2.0.15 builds a DTB segment from `/proc/device-tree`.
With `CONFIG_ARM_APPENDED_DTB=y`, the decompressor's `atags_to_fdt()` checks if r2
points to FDT_MAGIC — if so, returns 0 (no-op). The **appended DTB overwrites r2**
regardless (line 288 of `head.S`: `mov r8, r6`), discarding the external DTB's
`linux,initrd-start`/`linux,initrd-end` properties. The kernel boots without initramfs.

**The fix**: `--atags` forces kexec-tools to create ATAG_CORE + ATAG_CMDLINE +
ATAG_INITRD2 instead of a DTB. `atags_to_fdt()` then processes the ATAG list and
merges initrd info into the appended DTB correctly.

**Source analysis** (`vendor/kernel/arch/arm/boot/compressed/`):
- `head.S:267-270`: calls `atags_to_fdt(r8, r6)` where r8=boot params, r6=appended DTB
- `head.S:288`: `mov r8, r6` — appended DTB **always** wins
- `atags_to_fdt.c:96-97`: `if (*(u32 *)atag_list == FDT_MAGIC) return 0;` — THE BUG
- `atags_to_fdt.c:124-131`: ATAG_INITRD2 handler correctly sets linux,initrd-start/end

**Steam Link comparison**: Their `run.sh` uses `--dtb` as a separate file with a
**plain zImage** (no appended DTB). Our vendor zImage has appended DTB, so `--dtb`
would be ignored by the decompressor.

### Kexec Load Debug Output (with --atags)
```
MEMORY RANGES: 0000000002000000-000000001edfffff (0)
zImage header: 0x016f2818 0x00000000 0x0035d130
zImage size 0x35d130, file size 0x35d130
nr_segments = 3
segment[0]: ATAGS    312 bytes  at 0x02001000
segment[1]: kernel   3.5 MB     at 0x02008000
segment[2]: initrd   1.0 MB     at 0x030da000
```

### WiFi Hardware Limitation
The Marvell 88W8887 WiFi/BT combo chip connects via SDIO. After kexec, WiFi is
permanently lost until **full power cycle** — the WDT warm reboot alone does not
restore the SDIO chip. This means we cannot SSH into any kexec'd kernel.

### Test 1: First --atags test (yaffs2 mount in init)
- **Result**: "Absolutely instant reboot" — behavior completely different from all
  previous tests without --atags
- **Device boot-looped** — had to reflash
- **Root cause**: Init script mounted yaffs2 on mtd11 which was not cleanly unmounted
  before kexec. `machine_shutdown()` does NOT sync/unmount filesystems. Remounting
  dirty yaffs2 after kexec caused corruption that the stock boot couldn't recover from.
- **Conclusion**: **Proves init script executed** (yaffs2 corruption requires mount)
- **Fix**: NEVER mount yaffs2 (mtd11) in the kexec initramfs

### Test 2: After reflash, with NAND evidence write
- **Init script**: mknod + devmem WDT + dd to mtdblock13 at offset 0x2000
- **Result**: No evidence in mtd13 at offset 0x2000 (all 0xFF). /lsync intact (no corruption).
- **Discovery**: `CONFIG_DEVTMPFS is not set` in vendor kernel. The original
  `mount -t devtmpfs devtmpfs /dev` silently fails. All subsequent mknod, devmem, dd
  fail because /dev doesn't exist.
- **Fix**: Changed to `mount -t tmpfs tmpfs /dev` + manual `mknod` calls

### Test 3: With manual mknod, dd seek=16
- **Init script**: tmpfs /dev, mknod /dev/null + /dev/mem + /dev/mtdblock13,
  dd to mtdblock13 bs=512 seek=16 (preserving 8KB CENV header)
- **Result**: No evidence at mtd13 offset 0x2000. CENV data at offset 0 intact.
- **Hypothesis**: mtdblock erase-before-write on single 128KB erase block might not
  handle partial writes via seek, OR NAND controller not initialized after kexec

### DRAM Persistence Experiment (tests 4-6)

Since NAND writes were unreliable, switched to writing evidence markers to **physical
RAM** via BusyBox `devmem`. DRAM content should survive a WDT warm reset because the
retention time (~64ms at room temperature) outlasts the DDR controller re-initialization
during bootloader startup (~1ms).

**Methodology**: Write known marker values to RAM addresses both **before** kexec
(from stock system, confirmed working) and **in** the init script (the code under test).
After WDT reboot + stock boot, read back the addresses to determine:
- Did the pre-kexec marker survive? (proves DRAM persistence)
- Was it overwritten by the init marker? (proves init ran)
- Was it overwritten by something else? (boot process clobbers that address)

### DRAM Persistence Results

**Pre-kexec markers survive with predictable bit decay:**

| Address | Pre-kexec | Init writes | After reboot | Analysis |
|---------|-----------|-------------|--------------|----------|
| 0x08000000 | 0xBBBBBBBB | 0x4B584254 | 0xBFFFFFFB | Decayed pre-kexec marker |
| 0x10000000 | 0xBBBBBBBB | 0x4B584254 | 0xBFFFFFBB | Decayed pre-kexec marker |
| 0x14000000 | 0xBBBBBBBB | 0x4B584254 | 0xFFFBFFBF | Decayed pre-kexec marker |

**Bit decay pattern**: 0-bits in 0xBBBBBBBB (binary 10111011...) flip to 1 at a
~60-75% rate. This is textbook DRAM retention decay — cells with a preferred-1 state
flip 0->1 when the DDR controller briefly stops refreshing during warm reset.

**Conclusions from DRAM experiment:**
1. **DRAM partially survives WDT warm reset** — the bit-decay pattern is unmistakable
2. **The bootloader/kernel does NOT overwrite addresses 0x04-0x18 in the managed range**
3. **The init script is NOT executing its devmem writes** — the after-reboot values
   are consistent with decayed 0xBBBBBBBB (pre-kexec), NOT decayed 0x4B584254 (init)
4. **devmem write/readback works on the stock system** — confirmed by writing 0x4B584254
   to 0x08000000 and reading it back immediately (returns exact value)
5. **Bootloader overwrites 0x1EE00000+** — addresses above the kernel's memory range
   (0x02000000-0x1EDFFFFF) contain ARM bootloader code (confirmed: 0x1F000000 shows
   `mov r0, #1; cmp r0, #0; beq ...` — a polling/mode-switch sequence)

### Module Diagnostic Markers (in progress)

To determine whether the kexec module's execute path completes vs the new kernel
failing to boot, we added RAM markers to `machine_kexec()`:

- **Marker 1** (0xAAAAAAAA): Written via kernel VA (`phys_to_virt(0x08000000)`)
  before `local_irq_disable()` — tests that machine_kexec() reaches point-of-no-return
- **Marker 2** (0x55555555): Written via physical address after TTBR0 switch to
  idmap_pgd + identity mapping of 0x08000000 — tests that identity mapping works
  through to the final jump

Expected results after reboot:
- 0xBBBBBBBB (decayed) = machine_kexec didn't reach marker 1
- 0xAAAAAAAA (decayed) = marker 1 hit, TTBR0 switch or identity mapping failed
- 0x55555555 (decayed) = both module markers hit, new kernel doesn't boot or init fails

### Open Questions

1. **Why doesn't the init script execute?** The kexec module confirms through to
   `write_cp15_diag()` (file written to /lsync). Test 1 proved init CAN run (yaffs2
   corruption). But DRAM markers from tests 4-6 show the init's devmem writes don't
   happen. Possible causes:
   - Kernel decompressor fails (cache coherency, GIC state after kexec)
   - Kernel boots but doesn't find the initramfs (ATAG merge issue)
   - Kernel finds initramfs but can't execute /init (cpio format, permissions)
   - Init starts but crashes before devmem writes (BusyBox env issue)

2. **Was test 1's yaffs2 corruption really from the init script?** It could also be
   from the kexec shutdown leaving NAND in a dirty state. But tests 2-6 don't show
   corruption (yaffs2 mount removed from init), which suggests the kexec shutdown alone
   is safe and the corruption in test 1 was from the init script's explicit mount.

3. **Can we get serial console?** ttyS0 at 115200 baud would immediately reveal the
   full boot sequence. This is the single most valuable debugging step.

### Kernel Config (relevant to kexec boot)
```
CONFIG_BLK_DEV_INITRD=y          # kernel supports initrd/initramfs
CONFIG_INITRAMFS_SOURCE=""        # no built-in initramfs
CONFIG_RD_GZIP=y                  # can decompress gzip initramfs
CONFIG_ARM_APPENDED_DTB=y         # DTB appended to zImage
CONFIG_ARM_ATAG_DTB_COMPAT=y      # ATAG-to-DTB merging in decompressor
# CONFIG_DEVTMPFS is not set      # MUST use manual mknod in initramfs
# CONFIG_STRICT_DEVMEM is not set # devmem can write to any physical address
```

### Init Script (current minimal version)
```sh
#!/bin/sh
mount -t tmpfs tmpfs /dev
mknod /dev/mem c 1 1
/bin/devmem 0x08000000 32 0x4B584254
/bin/devmem 0x10000000 32 0x4B584254
/bin/devmem 0x14000000 32 0x4B584254
```

Source: `firmware/initramfs_tiny/init`
Build: `cd firmware/initramfs_tiny && find . | cpio -o -H newc | gzip -9 > ../initramfs_tiny.cpio.gz`
(Must build as root from native ext4, NOT NTFS — cpio ownership matters)

## Watchdog Considerations

The Harman Kardon Invoke has a DesignWare WDT at `0xF7FC2000` (CRR at offset `0x0C`).
Writing `0x76` to the CRR restarts the countdown. The module pre-maps the CRR at
init time via `ioremap()` so it's available during the kexec sequence.

**WDT pet strategy across the transition:**
1. **machine_kexec()** — pets WDT at max timeout before IRQ disable (ioremap'd VA)
2. **relocate_new_kernel** — pets WDT via direct physical MMIO (MMU off)
3. **6.1 kernel init script** — pets WDT via busybox `devmem 0xf7fc200c 32 0x76`

**Safety:** The WDT provides automatic recovery. If anything goes wrong during kexec,
the new kernel's boot, or the init script, the WDT fires after ~20-30s and reboots
to the encrypted stock kernel. The device can never be bricked by a bad kexec attempt.

## ARM MMU Reference (BG2CDP Specific)

For anyone debugging identity mapping issues on this platform:

| Register | Value | Meaning |
|----------|-------|---------|
| PHYS_OFFSET | `0x01100000` | RAM starts here (confirmed on device) |
| PAGE_OFFSET | `0xC0000000` | Kernel VA space starts here |
| SCTLR | `0x10c5387d` | TRE=1 (TEX remap active) — confirmed on device |
| TTBCR.N | `0` | TTBR0 handles all 4GB (TTBR1 not used for translation) |
| TTB_FLAGS_SMP | `0x6A` | Must be OR'd into TTBR0 for proper walker cache policy |
| TTBR0 (normal) | `0x1a87c06a` | Process page table (varies per boot/context) |
| TTBR0 (idmap) | `0x1e85006a` | idmap_pgd after setup_mm_for_reboot() — confirmed |
| idmap_pgd (phys) | `0x1e850000` | **NOT** 0xc062ac30 (System.map is wrong for running firmware) |
| idmap_pgd (virt) | `0xdd750000` | Via phys_to_virt (PHYS_OFFSET=0x01100000) |
| setup_mm_for_reboot | `0xc0019478` | Switches TTBR0 to idmap_pgd (via kallsyms) |
| DACR (default) | `0x00000015` | Confirmed on device (not 0x55555555) |
| PRRR | `0xff0a81a8` | TEX remap: primary region remap register — confirmed |
| NMRR | `0x40e040e0` | TEX remap: normal memory remap register — confirmed |
| PMD section desc | `0x40E` | PMD_TYPE_SECT \| PMD_SECT_AP_WRITE \| C \| B |

**Section descriptor layout (0x40E — our identity mapping)**:
```
bit  [1:0]  = 10    Section type
bit  [2]    = 1     B (bufferable) = 1   ← CRITICAL
bit  [3]    = 1     C (cacheable) = 1    ← CRITICAL
bit  [4]    = 0     XN (execute never) = 0
bit  [8:5]  = 0000  Domain 0
bit  [10]   = 1     AP[0] = 1  ┐
bit  [11]   = 0     AP[1] = 0  ├ AP=001: privileged RW
bit  [15]   = 0     AP[2] = 0  ┘
bit  [14:12]= 000   TEX[2:0] = 000
bit  [31:20]= phys  Base address (1MB aligned)
```

**C and B bits are critical**: Under TRE (TEX Remap Enable, SCTLR.TRE=1), the
TEX/C/B bits are remapped via PRRR/NMRR registers:
- TEX=000, C=0, B=0 → PRRR index 0 → **Strongly Ordered** → Cortex-A7 **CANNOT**
  fetch instructions (Prefetch Abort regardless of XN bit)
- TEX=000, C=1, B=1 → PRRR index 3 → **Normal WB-WA** → instruction fetch OK

The kernel's `identity_mapping_add()` uses 0x402 (Strongly Ordered) for `.idmap.text`
but that code only runs after `cpu_proc_fin()` has disabled caches. Our kexec skips
`cpu_proc_fin()` (to avoid ICIALLUIS hang) so we MUST use Normal memory (0x40E).

## Testing

### Identity Mapping Test (Non-Destructive)

The module provides a `KEXEC_IOCTL_TEST` ioctl (`0x4B455854`, 'KEXT') for non-destructive
identity mapping tests. Set `kexec_dry_run` to control test depth:

```bash
# On device:
insmod /lsync/kexec/kexec-mod.ko kexec_dry_run=3
/lsync/kexec/kexec-test          # triggers KEXEC_IOCTL_TEST ioctl
dmesg | grep kexec-test           # check results
```

The `kexec-test` binary (cross-compiled static ARM) opens `/dev/kexec` and calls the
test ioctl. Source: `tools/kexec-module/kexec-test.c`.

Test levels (controlled by `kexec_dry_run` parameter):
- **Level 1**: TTBR0 switch to idmap_pgd and back
- **Level 2**: + add identity mapping, read from physical address
- **Level 3**: + execute NOP+BX_LR at physical address

### CRITICAL: printk Deadlock with IRQs Disabled (2026-02-22)

**Never call `printk()` between `local_irq_disable()` and `local_irq_enable()`** in
the kexec module. On this SMP kernel, `printk` acquires `logbuf_lock` (a raw spinlock).
The WLAN driver generates constant scan messages (~every 7 seconds), so the other CPU
is frequently inside `printk` holding `logbuf_lock`. With IRQs disabled on our CPU,
we spin forever waiting for the lock — **deadlock**.

The test function buffers all diagnostic values in a `struct idmap_trace` during the
IRQ-disabled section, then prints everything after `local_irq_enable()`.

This also affects `machine_kexec()` — no printk calls should exist in the path between
`local_irq_disable()` and the jump to `relocate_new_kernel`.

### idmap_pgd Hardcoded Address (0xc062ac30) is WRONG

The System.map address `0xc062ac30` for `idmap_pgd` does NOT match the running kernel
firmware. Reading from that address produces garbage values (e.g., `0x5412b3c1`,
`0xc266f572` — different each boot). The vendor kernel source tree's System.map was
built from a different configuration than what's flashed on the device.

`setup_mm_for_reboot` at `0xc0019478` IS correct (found via kallsyms). The test function
avoids the hardcoded `idmap_pgd` entirely — it calls `setup_mm_for_reboot()` (which knows
the real `idmap_pgd`) then reads TTBR0 to discover the physical address.

### Test Script (Full kexec)
```bash
bash scripts/kexec/test_kexec.sh 192.168.43.1 3a   # dry-run level 1 (MM switch)
bash scripts/kexec/test_kexec.sh 192.168.43.1 3b   # dry-run level 2 (+ read)
bash scripts/kexec/test_kexec.sh 192.168.43.1 3c   # dry-run level 3 (+ exec)
bash scripts/kexec/test_kexec.sh 192.168.43.1 4    # real kexec (reboots device)
```

### Expected dmesg for Successful Test (via ioctl)
```
kexec-test: === PROGRESSIVE TEST level=3 ===
kexec-test: SCTLR=0x10c5387d TRE=1 PRRR=0xff0a81a8 NMRR=0x40e040e0
kexec-test: test page virt=0xXXXXXXXX phys=0xNNNNN000
kexec-test: entering IRQ-disabled section (NO printk until done)
kexec-test: === RESULTS (last step=9) ===
kexec-test: saved TTBR0=0xXXXXXXXX DACR=0x00000015
kexec-test: idmap TTBR0=0xXXXXXXXX hw_l1=0xXXXXXXXX
kexec-test: sect[N] old=0xXXXXXXXX new=0xNNNNN40e
kexec-test: READ phys 0xNNNNN000 = 0xe1a00000 MATCH
kexec-test: EXEC at phys 0xNNNNN000 PASSED
kexec-test: level=3 result=0 SUCCESS
```

## Files

| File | Purpose |
|------|---------|
| `tools/kexec-module/kernel/kexec-mod.h` | Force-defines CONFIG_KEXEC=1 |
| `tools/kexec-module/kernel/kexec-drv.c` | /dev/kexec driver, kallsyms resolution, WDT ioremap, ioctl test |
| `tools/kexec-module/kernel/kexec.c` | Wrapper around vendor's kernel/kexec.c |
| `tools/kexec-module/kernel/machine_kexec.c` | Custom ARM kexec: identity mapping, TTBR0 switch |
| `tools/kexec-module/kernel/relocate_kernel.S` | Custom trampoline: WDT pet, local cache flush, MMU off |
| `tools/kexec-module/kernel/Kbuild` | Kernel build file |
| `tools/kexec-module/kernel/Makefile` | Module build instructions |
| `tools/kexec-module/kexec-test.c` | Source for ioctl test binary (static ARM) |
| `tools/kexec-module/user/redir.c` | LD_PRELOAD shim (not needed with patched kexec) |
| `tools/kexec-module/user/Makefile` | redir.so build |
| `tools/kexec-tools/kexec-tools-2.0.15/` | Patched kexec-tools source |
| `scripts/build/build_kexec.sh` | Build script for all components |
| `scripts/kexec/deploy_kexec.sh` | Deploy to device |
| `scripts/kexec/test_kexec.sh` | Progressive test script (dry-run levels 1-3 + real) |
