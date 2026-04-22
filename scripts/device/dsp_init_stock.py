#!/usr/bin/env python3
"""
dsp_init_stock.py - Reproduces the EXACT stock dsp-client DSP initialization
sequence with full logging. Acts as a proxy/diagnostic to identify differences
between stock and Encore behavior.

Run on device:
  /lsync/scripts/py dsp_init_stock.py           # full init + poll for BOOTUP
  /lsync/scripts/py dsp_init_stock.py --dump     # just dump register state
  /lsync/scripts/py dsp_init_stock.py --upload-only  # init without poll

Requires: Encore DISABLED (touch /lsync/encore/disabled + reboot)
"""

import os, sys, struct, time, fcntl, mmap as _mmap, signal

# ─── Device constants ───────────────────────────────────────────────
I2C_BUS     = "/dev/i2c-0"
I2C_SLAVE   = 0x0703
IO_EXP_ADDR = 0x20
SPI_DEVICE  = "/dev/spidev0.0"
GPIO_BASE   = "/sys/class/gpio"

# SPI ioctl numbers (ARM32)
SPI_IOC_WR_MODE  = 0x40016b01
SPI_IOC_RD_MODE  = 0x80016b01
SPI_IOC_WR_BITS  = 0x40016b03
SPI_IOC_RD_BITS  = 0x80016b03
SPI_IOC_WR_SPEED = 0x40046b04
SPI_IOC_RD_SPEED = 0x80046b04
SPI_IOC_MESSAGE  = 0x40206b00  # SPI_IOC_MESSAGE(1) on ARM32

# AVIO register addresses
AUDIO_CLK = 0xF7EA8008
AVIO_400  = 0xF7E80400
AVIO_404  = 0xF7E80404
AVIO_450  = 0xF7E80450

# Firmware
FW_PATHS = ["/media/usb/dsp-img.ldr", "/data/test/dsp-img.ldr",
            "/usr/share/dsp/dsp-img.ldr"]
BLOCK_SIZE   = 1536      # boot kernel boundary
UPLOAD_SPEED = 1000000   # 1 MHz for boot kernel
NORMAL_SPEED = 58824     # messaging speed

# Bit-reverse LUT
BIT_REV = bytes([int(f'{b:08b}'[::-1], 2) for b in range(256)])


# ─── /dev/mem helpers ───────────────────────────────────────────────

def devmem_read(addr):
    """Read a 32-bit register via /dev/mem mmap."""
    fd = os.open("/dev/mem", os.O_RDONLY | os.O_SYNC)
    try:
        page = addr & ~0xFFF
        off  = addr & 0xFFF
        m = _mmap.mmap(fd, 4096, _mmap.MAP_SHARED, _mmap.PROT_READ, offset=page)
        val = struct.unpack_from("<I", m, off)[0]
        m.close()
        return val
    finally:
        os.close(fd)

def devmem_write(addr, val):
    """Write a 32-bit register via /dev/mem mmap."""
    fd = os.open("/dev/mem", os.O_RDWR | os.O_SYNC)
    try:
        page = addr & ~0xFFF
        off  = addr & 0xFFF
        m = _mmap.mmap(fd, 4096, _mmap.MAP_SHARED,
                       _mmap.PROT_READ | _mmap.PROT_WRITE, offset=page)
        struct.pack_into("<I", m, off, val)
        m.close()
    finally:
        os.close(fd)

def devmem_rmw(addr, set_bits=0, clear_bits=0, label=""):
    """Read-modify-write: set and clear specific bits. Returns old value."""
    old = devmem_read(addr)
    new = (old | set_bits) & ~clear_bits
    if new != old:
        devmem_write(addr, new)
    print(f"  AVIO 0x{addr:08X}: 0x{old:08X} -> 0x{new:08X}  {label}")
    return old


# ─── GPIO sysfs helpers ────────────────────────────────────────────

def gpio_export(pin):
    try:
        with open(f"{GPIO_BASE}/export", "w") as f:
            f.write(str(pin))
        print(f"  GPIO {pin}: exported")
    except OSError:
        print(f"  GPIO {pin}: already exported")

def gpio_unexport(pin):
    try:
        with open(f"{GPIO_BASE}/unexport", "w") as f:
            f.write(str(pin))
        print(f"  GPIO {pin}: unexported")
    except OSError:
        print(f"  GPIO {pin}: unexport failed")

def gpio_direction(pin, output):
    d = "out" if output else "in"
    with open(f"{GPIO_BASE}/gpio{pin}/direction", "w") as f:
        f.write(d)
    print(f"  GPIO {pin}: direction={d}")

def gpio_write(pin, val):
    with open(f"{GPIO_BASE}/gpio{pin}/value", "w") as f:
        f.write("1" if val else "0")
    print(f"  GPIO {pin}: value={'1' if val else '0'}")

def gpio_read(pin):
    with open(f"{GPIO_BASE}/gpio{pin}/value", "r") as f:
        v = f.read().strip()
    print(f"  GPIO {pin}: read={v}")
    return int(v)


# ─── I2C helpers ────────────────────────────────────────────────────

def i2c_write_reg(addr, reg, val):
    """Write one byte to an I2C register."""
    fd = os.open(I2C_BUS, os.O_RDWR)
    try:
        fcntl.ioctl(fd, I2C_SLAVE, addr)
        os.write(fd, bytes([reg, val]))
    finally:
        os.close(fd)

def i2c_read_reg(addr, reg, timeout=3):
    """Read one byte from an I2C register.
    Uses fork + timeout to prevent kernel-level I2C hang when AVIO bus is dead.
    """
    # Create pipe for child to send result back
    r_fd, w_fd = os.pipe()
    pid = os.fork()
    if pid == 0:
        # Child process — do the I2C read, send result via pipe, _exit
        os.close(r_fd)
        try:
            fd = os.open(I2C_BUS, os.O_RDWR)
            fcntl.ioctl(fd, I2C_SLAVE, addr)
            os.write(fd, bytes([reg]))
            data = os.read(fd, 1)
            os.close(fd)
            os.write(w_fd, data)
        except Exception:
            pass
        os.close(w_fd)
        os._exit(0)
    else:
        # Parent — wait with timeout
        os.close(w_fd)
        deadline = time.time() + timeout
        result = None
        while True:
            wpid, status = os.waitpid(pid, os.WNOHANG)
            if wpid != 0:
                # Child exited — read result
                try:
                    data = os.read(r_fd, 1)
                    if data:
                        result = data[0]
                except Exception:
                    pass
                break
            if time.time() > deadline:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
                os.close(r_fd)
                raise TimeoutError(f"I2C read 0x{addr:02X} reg 0x{reg:02X} hung (bus dead?)")
            time.sleep(0.05)
        os.close(r_fd)
        if result is None:
            raise OSError(f"I2C read 0x{addr:02X} reg 0x{reg:02X} failed")
        return result


# ─── SPI helpers ────────────────────────────────────────────────────

class SPI:
    def __init__(self):
        self.fd = os.open(SPI_DEVICE, os.O_RDWR)
        # Set mode 3, 8 bits
        fcntl.ioctl(self.fd, SPI_IOC_WR_MODE, struct.pack("B", 3))
        fcntl.ioctl(self.fd, SPI_IOC_WR_BITS, struct.pack("B", 8))
        self.set_speed(UPLOAD_SPEED)
        print("  SPI: opened %s (mode=3, 8bit, %dHz)" % (SPI_DEVICE, UPLOAD_SPEED))

    def set_speed(self, hz):
        fcntl.ioctl(self.fd, SPI_IOC_WR_SPEED, struct.pack("<I", hz))
        print("  SPI: speed=%dHz" % hz)

    def read(self, n):
        """Half-duplex SPI read (sends zeros, returns rx data)."""
        return os.read(self.fd, n)

    def write(self, data):
        """Half-duplex SPI write (ignores rx)."""
        os.write(self.fd, data)

    def close(self):
        os.close(self.fd)


# ─── DAC diagnostic ────────────────────────────────────────────────

def dac_read(reg):
    """Read DAC (0x4C) register via I2C."""
    try:
        return i2c_read_reg(0x4C, reg)
    except Exception as e:
        print(f"  DAC: read reg 0x{reg:02X} failed: {e}")
        return None

def dac_select_page(page):
    i2c_write_reg(0x4C, 0x00, page)

def dump_dac_status():
    """Read DAC clock/auto-mute status registers."""
    try:
        dac_select_page(0)
        r5d = dac_read(0x5D)
        r5e = dac_read(0x5E)
        r02 = dac_read(0x02)
        if r5d is not None and r5e is not None and r02 is not None:
            print(f"  DAC status: 0x5D=0x{r5d:02X} 0x5E=0x{r5e:02X} 0x02=0x{r02:02X}")
            if r5d & 0x40:
                print(f"    -> I2S clocks PRESENT")
            else:
                print(f"    -> NO I2S clocks (DSP not generating output)")
        else:
            print(f"  DAC status: partial read failure")
    except (TimeoutError, OSError) as e:
        print(f"  DAC status: UNAVAILABLE ({e})")


# ─── Register dump ──────────────────────────────────────────────────

def dump_all_registers():
    """Dump all AVIO registers and DAC status."""
    print("\n=== Register Dump ===")
    for name, addr in [("AUDIO_CLK", AUDIO_CLK), ("AVIO_400", AVIO_400),
                       ("AVIO_404", AVIO_404), ("AVIO_450", AVIO_450)]:
        val = devmem_read(addr)
        print(f"  {name} (0x{addr:08X}) = 0x{val:08X}")
        if name == "AVIO_404":
            # Decode GPIO direction bits
            for pin in [4, 5, 12, 13, 14, 15]:
                bit = 1 << pin
                d = "OUTPUT" if (val & bit) else "input"
                print(f"    GPIO {pin}: {d}")
        elif name == "AVIO_400":
            for pin in [4, 5]:
                bit = 1 << pin
                v = "HIGH" if (val & bit) else "LOW"
                print(f"    GPIO {pin}: {v}")
    dump_dac_status()
    # IO Expander
    try:
        reg1 = i2c_read_reg(IO_EXP_ADDR, 1)
        print(f"  IO Expander reg 1 = 0x{reg1:02X}")
        print(f"    DSP reset  (bit 0): {'RELEASED' if reg1 & 1 else 'ASSERTED'}")
        print(f"    Amp mute   (bit 1): {'MUTED' if reg1 & 2 else 'unmuted'}")
        print(f"    DAC mute   (bit 2): {'unmuted' if reg1 & 4 else 'MUTED'}")
        print(f"    DSP power  (bit 4): {'ON' if reg1 & 16 else 'off'}")
    except Exception as e:
        print(f"  IO Expander: read failed: {e}")
    print()


# ─── Stock DSP initialization sequence ─────────────────────────────

def stock_dspopen():
    """
    Reproduces the exact stock dspopen() sequence.
    Returns SPI object for subsequent messaging.
    """
    print("\n=== Step 1: Open SPI device ===")
    spi = SPI()

    print("\n=== Step 2: AVIO clock enable (0xF7EA8008 bit 24) ===")
    # Stock: FUN_0008df90(0) — read nibble at gpioval[3], OR with 1 → bit 24
    devmem_rmw(AUDIO_CLK, set_bits=(1 << 24), label="set PLL bit 24")

    print("\n=== Step 3: AVIO 0x404 GPIO direction config ===")
    # Stock: read-modify-write each GPIO pin individually via devmem
    devmem_rmw(AVIO_404, set_bits=(1 << 4),  label="GPIO 4: output enable")
    devmem_rmw(AVIO_404, clear_bits=(1 << 13), label="GPIO 13: input")
    devmem_rmw(AVIO_404, clear_bits=(1 << 12), label="GPIO 12: input")
    devmem_rmw(AVIO_404, clear_bits=(1 << 15), label="GPIO 15: input")

    print("\n=== Step 4: AVIO 0x400 set GPIO 4 high ===")
    # Stock: read 0x400, set bit 4, write back
    devmem_rmw(AVIO_400, set_bits=(1 << 4), label="GPIO 4: HIGH")

    print("\n=== Step 5: Read GPIO input states via 0x450 ===")
    val_450 = devmem_read(AVIO_450)
    print(f"  AVIO 0x450 = 0x{val_450:08X}")
    for pin in [12, 13, 15]:
        v = "HIGH" if (val_450 & (1 << pin)) else "LOW"
        print(f"    GPIO {pin}: {v}")

    print("\n=== Step 6: sysfs GPIO export ===")
    for pin in [4, 13, 12, 15]:  # stock order
        gpio_export(pin)

    print("\n=== Step 7: sysfs GPIO directions ===")
    gpio_direction(4, True)   # output
    gpio_direction(13, False) # input
    gpio_direction(12, False) # input
    gpio_direction(15, False) # input

    print("\n=== Step 8: sysfs GPIO values ===")
    gpio_write(4, True)       # SPI CS idle HIGH
    gpio_read(13)
    gpio_read(12)
    gpio_read(15)

    print("\n=== Step 9: Pre-upload register state ===")
    dump_all_registers()

    print("\n=== Step 10: Firmware upload (stock FUN_0008e640) ===")
    stock_upload_firmware(spi)

    print("\n=== Step 11: Post-upload register state ===")
    dump_all_registers()

    return spi


def stock_upload_firmware(spi):
    """Reproduces stock FUN_0008e640 firmware upload sequence."""

    # Find firmware
    fw_path = None
    for p in FW_PATHS:
        if os.path.exists(p):
            fw_path = p
            break
    if not fw_path:
        print("  ERROR: DSP firmware not found!")
        return False

    # Read firmware
    with open(fw_path, "rb") as f:
        fw = bytearray(f.read())
    print(f"  Firmware: {fw_path} ({len(fw)} bytes)")

    # Bit-reverse every byte
    for i in range(len(fw)):
        fw[i] = BIT_REV[fw[i]]
    print(f"  Bit-reversed {len(fw)} bytes")

    # === DSP Reset ===
    print("\n  --- DSP Reset (I2C IO Expander 0x20) ---")
    reg1 = i2c_read_reg(IO_EXP_ADDR, 1)
    print(f"  IO Expander reg 1 before: 0x{reg1:02X}")

    # HIGH (deassert)
    i2c_write_reg(IO_EXP_ADDR, 1, reg1 | 0x01)
    print(f"  DSP reset: deassert (bit 0 = 1)")

    # LOW (assert reset)
    i2c_write_reg(IO_EXP_ADDR, 1, (reg1 | 0x01) & 0xFE)
    print(f"  DSP reset: ASSERT (bit 0 = 0)")
    time.sleep(0.020)  # 20ms

    # HIGH (release reset)
    i2c_write_reg(IO_EXP_ADDR, 1, reg1 | 0x01)
    print(f"  DSP reset: release (bit 0 = 1)")
    time.sleep(0.010)  # 10ms

    reg1_after = i2c_read_reg(IO_EXP_ADDR, 1)
    print(f"  IO Expander reg 1 after: 0x{reg1_after:02X}")

    # === AVIO config for upload ===
    print("\n  --- AVIO config for upload ---")

    # Stock: FUN_0008df90(1) — set bit 27 AND bit 24 of 0xF7EA8008
    # This is the CRITICAL difference from Encore which only sets bit 24
    devmem_rmw(AUDIO_CLK, set_bits=(1 << 27) | (1 << 24),
               label="UPLOAD clock: bits 24+27")

    # Stock: FUN_0008e1f0(5,0) — enable GPIO 5 as output in 0x404
    devmem_rmw(AVIO_404, set_bits=(1 << 5),
               label="GPIO 5: output enable")

    # Stock: read 0x400, clear bit 5 (GPIO 5 LOW via AVIO), write back
    devmem_rmw(AVIO_400, clear_bits=(1 << 5),
               label="GPIO 5: LOW via AVIO")

    # === GPIO 5 sysfs setup + pulse ===
    print("\n  --- GPIO 5 pulse ---")
    gpio_export(5)
    gpio_direction(5, True)   # output
    gpio_write(5, True)       # HIGH
    gpio_write(5, False)      # LOW (start upload pulse)

    # === SPI upload ===
    print("\n  --- SPI firmware transfer ---")
    saved_speed = NORMAL_SPEED
    spi.set_speed(UPLOAD_SPEED)

    bytes_sent = 0
    blocks_sent = 0
    for offset in range(0, len(fw), 4):
        end = min(offset + 4, len(fw))
        chunk = fw[offset:end]
        if len(chunk) < 4:
            chunk = chunk + b'\x00' * (4 - len(chunk))
        spi.write(bytes(chunk))
        bytes_sent = offset + 4

        if bytes_sent % BLOCK_SIZE == 0:
            blocks_sent += 1
            if blocks_sent == 1:
                # Stock: restore normal speed after first 1536-byte block
                spi.set_speed(NORMAL_SPEED)
                time.sleep(0.010)  # 10ms
                print(f"  Uploaded boot kernel ({BLOCK_SIZE} bytes), speed dropped")

    print(f"  Upload complete: {bytes_sent} bytes, {blocks_sent} blocks")

    # === Post-upload ===
    print("\n  --- Post-upload GPIO + AVIO ---")
    gpio_write(5, True)       # deassert upload CS
    gpio_unexport(5)

    # Stock: FUN_0008df90(0) — restore 0xF7EA8008 to bit 24 only (clear bit 27)
    devmem_rmw(AUDIO_CLK, set_bits=(1 << 24), clear_bits=(1 << 27),
               label="RESTORE clock: bit 24 only")

    print(f"  Firmware upload sequence complete")
    return True


# ─── SPI message protocol (post-upload) ────────────────────────────

def build_spi_message(msg_type, data):
    """Build SPI message: [type_hi, type_lo, len_hi, len_lo, checksum, data, pad to 8]"""
    th = (msg_type >> 8) & 0xFF
    tl = msg_type & 0xFF
    lh = (len(data) >> 8) & 0xFF
    ll = len(data) & 0xFF
    chk = (th + tl + lh + ll + sum(data)) & 0xFF
    msg = bytes([th, tl, lh, ll, chk]) + bytes(data)
    while len(msg) % 8 != 0:
        msg += b'\x00'
    return msg


def poll_dsp_events(spi, timeout_s=30):
    """
    Poll for DSP events using GPIO handshaking (stock msgproc behavior).
    Polls GPIO 15 for DSP ready, GPIO 12 for data available.
    """
    print(f"\n=== Polling for DSP events ({timeout_s}s timeout) ===")
    print(f"  (Stock polls every 200ms with GPIO handshake)")
    start = time.time()
    poll_count = 0
    rx_buf = bytearray(2048)

    while time.time() - start < timeout_s:
        poll_count += 1

        # Check GPIO 15 (DSP ready)
        try:
            gpio15 = gpio_read(15) if poll_count <= 3 else _gpio_read_silent(15)
        except Exception:
            gpio15 = 0

        # Check GPIO 12 (data available)
        try:
            gpio12 = gpio_read(12) if poll_count <= 3 else _gpio_read_silent(12)
        except Exception:
            gpio12 = 0

        if poll_count <= 5:
            print(f"  Poll #{poll_count}: GPIO12={gpio12} GPIO15={gpio15}")

        # If data available, try to read
        if gpio12 or (poll_count % 5 == 0):
            # Stock handshake:
            # 1. Set GPIO 13 = output, drive LOW
            try:
                gpio_direction(13, True) if poll_count == 1 else _gpio_dir_silent(13, True)
                _gpio_write_silent(13, False)
            except Exception:
                pass

            # 2. Toggle GPIO 4 LOW then HIGH (chip select pulse)
            try:
                _gpio_write_silent(4, False)
                time.sleep(0.000001)  # 1us
                _gpio_write_silent(4, True)
            except Exception:
                pass

            # 3. SPI read (write zeros to clock data out of DSP)
            try:
                spi.write(b'\x00' * 2048)
                rx = spi.read(2048)
                # Check for non-zero data
                nonzero = [i for i, b in enumerate(rx) if b != 0]
                if nonzero:
                    start_idx = nonzero[0]
                    end_idx = nonzero[-1] + 1
                    data = rx[start_idx:end_idx]
                    hex_str = ' '.join('0x%02X' % b for b in data[:32])
                    print("  *** SPI RESPONSE at offset %d: %s" % (start_idx, hex_str))

                    # Parse event
                    if len(data) >= 3:
                        cat = (data[0] << 8) | data[1]
                        code = data[2]
                        print("  *** Event: category=0x%04X code=0x%02X" % (cat, code))
                        if cat == 1 and code == 4:
                            print("\n  *** EVENT_DSP_BOOTUP RECEIVED! ***")
                            dump_dac_status()
                            return True
                        elif cat == 0 and code == 8:
                            print("  *** DSP Version response")
                        elif cat == 0 and code == 4:
                            print("  *** DAC Gain event")
            except Exception as e:
                if poll_count <= 3:
                    print("  SPI read error: %s" % e)

            # 4. Restore GPIO 13 as input
            try:
                _gpio_dir_silent(13, False)
            except Exception:
                pass

        # 200ms poll interval (matches stock)
        time.sleep(0.200)

        # Periodic status
        if poll_count % 25 == 0:
            elapsed = time.time() - start
            print(f"  ... {poll_count} polls, {elapsed:.1f}s elapsed")
            dump_dac_status()

    print(f"  Timeout after {poll_count} polls ({timeout_s}s)")
    return False


def _gpio_read_silent(pin):
    with open(f"{GPIO_BASE}/gpio{pin}/value", "r") as f:
        return int(f.read().strip())

def _gpio_write_silent(pin, val):
    with open(f"{GPIO_BASE}/gpio{pin}/value", "w") as f:
        f.write("1" if val else "0")

def _gpio_dir_silent(pin, output):
    with open(f"{GPIO_BASE}/gpio{pin}/direction", "w") as f:
        f.write("out" if output else "in")


# ─── Main ───────────────────────────────────────────────────────────

def main():
    args = sys.argv[1:]

    if "--dump" in args:
        # Enable AVIO clock first so I2C bus works
        print("Enabling AVIO PLL clock for I2C access...")
        devmem_rmw(AUDIO_CLK, set_bits=(1 << 24), label="set PLL bit 24")
        dump_all_registers()
        return

    print("=" * 60)
    print("Stock DSP Init Proxy")
    print("Reproduces exact stock dsp-client initialization sequence")
    print("=" * 60)

    # CRITICAL: Enable AVIO clock FIRST — without this, the I2C controller
    # at 0xF7E81400 is not clocked and any I2C read hangs in kernel space
    # indefinitely (uninterruptible D state → watchdog reset).
    print("\n=== Pre-init: Enable AVIO PLL clock ===")
    devmem_rmw(AUDIO_CLK, set_bits=(1 << 24), label="set PLL bit 24 (enable I2C bus)")

    print("\n=== Initial register state ===")
    dump_all_registers()

    # Run stock init sequence
    spi = stock_dspopen()

    if "--upload-only" in args:
        print("\n=== Upload-only mode, skipping poll ===")
        dump_dac_status()
        spi.close()
        return

    # Wait 2s for DSP to boot (stock immediately enters poll loop)
    print("\n=== Waiting 2s for DSP boot ===")
    time.sleep(2.0)
    dump_dac_status()

    # Poll for BOOTUP event
    bootup = poll_dsp_events(spi, timeout_s=30)

    if bootup:
        print("\n=== DSP BOOTED! Unmuting... ===")
        # Stock calls mcu-interface via WAMP to unmute
        # We do it directly via I2C
        reg1 = i2c_read_reg(IO_EXP_ADDR, 1)
        # Unmute DAC (bit 2 = 1 for unmute, active-low)
        i2c_write_reg(IO_EXP_ADDR, 1, reg1 | 0x04)
        print(f"  DAC unmuted")
        time.sleep(0.100)  # 100ms (stock delay)
        # Unmute amp (bit 1 = 0 for unmute)
        reg1 = i2c_read_reg(IO_EXP_ADDR, 1)
        i2c_write_reg(IO_EXP_ADDR, 1, reg1 & ~0x02)
        print(f"  Amp unmuted")
    else:
        print("\n=== DSP did NOT send BOOTUP event ===")

    print("\n=== Final register state ===")
    dump_all_registers()

    spi.close()
    print("\nDone.")


if __name__ == "__main__":
    main()
