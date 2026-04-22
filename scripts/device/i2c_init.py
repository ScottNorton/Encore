#!/usr/bin/env python3
"""Initialize AVIO I2C controller and test MCU communication.

When Encore is disabled, the AVIO block I2C controller at 0xF7E81400
is not initialized, causing all I2C devices (MCU 0x36, DAC 0x4C,
IO Expander 0x20) to time out. This script writes the same SoC
registers that Encore's DSP subsystem writes to enable the bus.

I2C probes use fork+kill to avoid hanging the main process --
DesignWare I2C reads to a dead controller block in kernel space
and SIGALRM cannot interrupt them.
"""
import os
import sys
import struct
import mmap
import time
import fcntl

# --- SoC register addresses (from Encore dsp.rs) ---
AVIO_CONFIG   = 0xF7E80400
AVIO_SPI_MODE = 0xF7E80404
AUDIO_CLK     = 0xF7EA8008

# Values Encore writes
AVIO_CONFIG_VAL   = 0x00000A08
AVIO_SPI_MODE_VAL = 0x00000F28
AUDIO_CLK_VAL     = 0x0118D249

KNOWN_DEVICES = {
    0x19: "PMIC 88PG868",
    0x1A: "WM8904 codec",
    0x20: "IO Expander",
    0x36: "MCU MSP430",
    0x4C: "DAC TAS5756M",
}

PAGE_SIZE = 4096
I2C_SLAVE = 0x0703

def out(msg):
    sys.stdout.write(msg + "\n")
    sys.stdout.flush()

def devmem_read(addr):
    page = addr & ~(PAGE_SIZE - 1)
    offset = addr & (PAGE_SIZE - 1)
    fd = os.open("/dev/mem", os.O_RDONLY | os.O_SYNC)
    try:
        m = mmap.mmap(fd, PAGE_SIZE, mmap.MAP_SHARED, mmap.PROT_READ, offset=page)
        val = struct.unpack_from("<I", m, offset)[0]
        m.close()
        return val
    finally:
        os.close(fd)

def devmem_write(addr, value):
    page = addr & ~(PAGE_SIZE - 1)
    offset = addr & (PAGE_SIZE - 1)
    fd = os.open("/dev/mem", os.O_RDWR | os.O_SYNC)
    try:
        m = mmap.mmap(fd, PAGE_SIZE, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE, offset=page)
        struct.pack_into("<I", m, offset, value)
        m.close()
    finally:
        os.close(fd)

def i2c_probe_child(bus, addr):
    """Run in forked child: try I2C read, exit 0=ACK, 1=NACK/error."""
    try:
        fd = os.open("/dev/i2c-%d" % bus, os.O_RDWR)
        fcntl.ioctl(fd, I2C_SLAVE, addr)
        os.read(fd, 1)
        os.close(fd)
        os._exit(0)  # ACK
    except:
        os._exit(1)  # NACK or error

def i2c_probe(bus, addr, timeout_s=3):
    """Fork a child to probe I2C. Kill it if it doesn't return in time."""
    pid = os.fork()
    if pid == 0:
        # Child
        i2c_probe_child(bus, addr)
    # Parent
    deadline = time.monotonic() + timeout_s
    while True:
        try:
            rpid, status = os.waitpid(pid, os.WNOHANG)
            if rpid != 0:
                if os.WIFEXITED(status):
                    return "ACK" if os.WEXITSTATUS(status) == 0 else "NACK"
                return "SIGNAL"
        except ChildProcessError:
            return "NACK"
        if time.monotonic() > deadline:
            try:
                os.kill(pid, 9)
                os.waitpid(pid, 0)
            except:
                pass
            return "TIMEOUT(killed)"
        time.sleep(0.05)

def i2c_read_child(bus, addr, write_bytes, read_len, pipe_w):
    """Forked child: write cmd, read response, send back via pipe."""
    try:
        fd = os.open("/dev/i2c-%d" % bus, os.O_RDWR)
        fcntl.ioctl(fd, I2C_SLAVE, addr)
        if write_bytes:
            os.write(fd, write_bytes)
            time.sleep(0.02)
        data = os.read(fd, read_len)
        os.write(pipe_w, data)
        os.close(fd)
        os._exit(0)
    except:
        os._exit(1)

def i2c_cmd(bus, addr, cmd_bytes, resp_len, timeout_s=3):
    """Fork a child to do I2C write+read. Returns bytes or None."""
    r, w = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(r)
        i2c_read_child(bus, addr, cmd_bytes, resp_len, w)
    os.close(w)
    deadline = time.monotonic() + timeout_s
    while True:
        try:
            rpid, status = os.waitpid(pid, os.WNOHANG)
            if rpid != 0:
                if os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0:
                    data = os.read(r, resp_len)
                    os.close(r)
                    return data
                os.close(r)
                return None
        except ChildProcessError:
            os.close(r)
            return None
        if time.monotonic() > deadline:
            try:
                os.kill(pid, 9)
                os.waitpid(pid, 0)
            except:
                pass
            os.close(r)
            return None
        time.sleep(0.05)

def show_i2c_adapters():
    out("=== I2C Adapters ===")
    try:
        entries = os.listdir("/sys/class/i2c-adapter")
    except OSError:
        entries = []
    adapters = sorted(e for e in entries if e.startswith("i2c-"))
    if not adapters:
        out("  No I2C adapters found!")
        return []
    bus_ids = []
    for name in adapters:
        bus_id = int(name.split("-")[1])
        bus_ids.append(bus_id)
        path = "/sys/class/i2c-adapter/" + name
        try:
            with open(path + "/name") as f:
                aname = f.read().strip()
        except:
            aname = "unknown"
        try:
            link = os.readlink(path)
        except:
            link = "?"
        out("  %s: name=%s" % (name, aname))
        out("         path=%s" % link)
    return bus_ids

def show_avio_regs(label="current"):
    out("\n=== AVIO Registers (%s) ===" % label)
    for addr, name in [(AVIO_CONFIG, "AVIO_CONFIG  "),
                       (AVIO_SPI_MODE, "AVIO_SPI_MODE"),
                       (AUDIO_CLK, "AUDIO_CLK    ")]:
        try:
            val = devmem_read(addr)
            out("  %s (0x%08X) = 0x%08X" % (name, addr, val))
        except Exception as e:
            out("  %s (0x%08X) = ERROR: %s" % (name, addr, e))

def init_avio():
    out("\n=== Writing AVIO registers ===")
    for addr, val, name in [
        (AUDIO_CLK,     AUDIO_CLK_VAL,     "AUDIO_CLK    "),
        (AVIO_SPI_MODE, AVIO_SPI_MODE_VAL, "AVIO_SPI_MODE"),
        (AVIO_CONFIG,   AVIO_CONFIG_VAL,   "AVIO_CONFIG  "),
    ]:
        old = devmem_read(addr)
        devmem_write(addr, val)
        new = devmem_read(addr)
        tag = "CHANGED" if old != new else "same"
        out("  %s: 0x%08X -> 0x%08X (%s)" % (name, old, new, tag))
    time.sleep(0.2)

def test_known_devices(bus):
    out("\n=== Known devices on bus %d (forked probes) ===" % bus)
    results = {}
    for addr in sorted(KNOWN_DEVICES.keys()):
        name = KNOWN_DEVICES[addr]
        r = i2c_probe(bus, addr)
        results[addr] = r
        out("  0x%02X %-16s -> %s" % (addr, name, r))
    return results

def test_mcu(bus):
    out("\n=== MCU version query (bus %d) ===" % bus)
    cmd = bytes([0x01, 0x00, 0x00, 0x00, 0x00, 0x00])
    resp = i2c_cmd(bus, 0x36, cmd, 6)
    if resp:
        hex_str = " ".join("%02x" % b for b in resp)
        out("  Response: %s" % hex_str)
        if len(resp) >= 3 and resp[0] == 0x01:
            out("  MCU firmware version: %d.%d" % (resp[1], resp[2]))
        return True
    else:
        out("  No response (timeout or error)")
        return False

def main():
    out("I2C Init & Diagnostic Tool (fork-safe)")
    out("=" * 45)

    buses = show_i2c_adapters()
    show_avio_regs("before init")

    # Quick pre-init MCU probe
    if buses:
        out("\n--- Pre-init MCU probe (bus %d) ---" % buses[0])
        r = i2c_probe(buses[0], 0x36)
        out("  MCU 0x36: %s" % r)

    if "--no-init" not in sys.argv:
        init_avio()
        show_avio_regs("after init")

        if buses:
            for b in buses:
                results = test_known_devices(b)
                if results.get(0x36) == "ACK":
                    test_mcu(b)
                    break
            else:
                out("\nMCU not responding on any bus after init.")
    else:
        out("\n(skipping init)")

    out("\nDone.")

if __name__ == "__main__":
    main()
