#!/usr/bin/env python3
"""Read/write SoC registers via /dev/mem. Like busybox devmem but in Python.

Usage:
    python3 devmem.py 0xF7E81400           # Read 32-bit register
    python3 devmem.py 0xF7E81400 0x1234    # Write 32-bit register
    python3 devmem.py i2c                  # Dump I2C controller registers
"""
import mmap
import os
import struct
import sys

PAGE_SIZE = 4096

def devmem_read(addr):
    fd = os.open("/dev/mem", os.O_RDWR | os.O_SYNC)
    page = addr & ~(PAGE_SIZE - 1)
    offset = addr & (PAGE_SIZE - 1)
    m = mmap.mmap(fd, PAGE_SIZE, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE, offset=page)
    val = struct.unpack("<I", m[offset:offset+4])[0]
    m.close()
    os.close(fd)
    return val

def devmem_write(addr, val):
    fd = os.open("/dev/mem", os.O_RDWR | os.O_SYNC)
    page = addr & ~(PAGE_SIZE - 1)
    offset = addr & (PAGE_SIZE - 1)
    m = mmap.mmap(fd, PAGE_SIZE, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE, offset=page)
    m[offset:offset+4] = struct.pack("<I", val)
    m.close()
    os.close(fd)

def dump_i2c():
    BASE = 0xF7E81400
    regs = [
        (0x00, "IC_CON"),
        (0x04, "IC_TAR"),
        (0x10, "IC_DATA_CMD"),
        (0x14, "IC_SS_SCL_HCNT"),
        (0x18, "IC_SS_SCL_LCNT"),
        (0x1C, "IC_FS_SCL_HCNT"),
        (0x20, "IC_FS_SCL_LCNT"),
        (0x2C, "IC_INTR_STAT"),
        (0x30, "IC_INTR_MASK"),
        (0x34, "IC_RAW_INTR_STAT"),
        (0x38, "IC_RX_TL"),
        (0x3C, "IC_TX_TL"),
        (0x40, "IC_CLR_INTR"),
        (0x6C, "IC_ENABLE"),
        (0x70, "IC_STATUS"),
        (0x74, "IC_TXFLR"),
        (0x78, "IC_RXFLR"),
        (0x7C, "IC_SDA_HOLD"),
        (0x80, "IC_TX_ABRT_SOURCE"),
        (0x9C, "IC_ENABLE_STATUS"),
        (0xA0, "IC_FS_SPKLEN"),
        (0xF4, "IC_COMP_PARAM_1"),
        (0xF8, "IC_COMP_VERSION"),
        (0xFC, "IC_COMP_TYPE"),
    ]
    print(f"DesignWare I2C controller at 0x{BASE:08X}:")
    for off, name in regs:
        try:
            val = devmem_read(BASE + off)
            print(f"  {name:25s} [0x{off:02X}] = 0x{val:08X}")
        except Exception as e:
            print(f"  {name:25s} [0x{off:02X}] = ERROR: {e}")

    # Also dump AVIO regs
    print()
    print("AVIO block:")
    for addr, name in [
        (0xF7E80400, "AVIO_CONFIG"),
        (0xF7E80404, "AVIO_SPI_MODE"),
    ]:
        try:
            val = devmem_read(addr)
            print(f"  {name:25s} = 0x{val:08X}")
        except Exception as e:
            print(f"  {name:25s} = ERROR: {e}")

    print()
    print("Audio clock:")
    try:
        val = devmem_read(0xF7EA8008)
        print(f"  AUDIO_CLK               = 0x{val:08X}")
    except Exception as e:
        print(f"  AUDIO_CLK               = ERROR: {e}")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)

    if sys.argv[1] == "i2c":
        dump_i2c()
    elif len(sys.argv) == 2:
        addr = int(sys.argv[1], 0)
        val = devmem_read(addr)
        print(f"0x{addr:08X} = 0x{val:08X}")
    elif len(sys.argv) == 3:
        addr = int(sys.argv[1], 0)
        val = int(sys.argv[2], 0)
        devmem_write(addr, val)
        readback = devmem_read(addr)
        print(f"0x{addr:08X} <- 0x{val:08X} (readback: 0x{readback:08X})")
