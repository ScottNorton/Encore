#!/usr/bin/env python3
"""Dump all 320 pages of ADSP-21489 SRAM via SPI.

Opens /dev/spidev0.0 directly. Safe to run alongside Encore as long as
no one triggers SPI commands (web UI DSP controls) during the dump.

Output: /lsync/dsp_dump.bin (raw concatenation of 320 SPI responses)
"""
import os, sys, struct, time, fcntl, array

SPI_DEV = "/dev/spidev0.0"
SPI_SPEED = 58824  # messaging speed (post-upload)
SPI_MODE = 3
SPI_BITS = 8
PAGES = 0x140  # 320 pages
RX_SIZE = 2048  # response buffer per page
OUT_PATH = "/lsync/dsp_dump.bin"

# SPI ioctl constants
SPI_IOC_WR_MODE = 0x40016B01
SPI_IOC_WR_BITS = 0x40016B03
SPI_IOC_WR_SPEED = 0x40046B04
SPI_IOC_MESSAGE_1 = 0x40206B00  # SPI_IOC_MESSAGE(1) on ARM32


def spi_transfer(fd, tx, rx_len):
    """Full-duplex SPI transfer using SPI_IOC_MESSAGE ioctl."""
    tx_buf = array.array('B', tx)
    rx_buf = array.array('B', [0] * rx_len)
    tx_addr = tx_buf.buffer_info()[0]
    rx_addr = rx_buf.buffer_info()[0]
    length = max(len(tx), rx_len)

    # Pad tx to match rx_len if needed
    if len(tx) < rx_len:
        tx_buf = array.array('B', tx + bytes(rx_len - len(tx)))
        tx_addr = tx_buf.buffer_info()[0]
        rx_buf = array.array('B', [0] * rx_len)
        rx_addr = rx_buf.buffer_info()[0]
        length = rx_len

    # struct spi_ioc_transfer (32 bytes on ARM32):
    #   u64 tx_buf, u64 rx_buf, u32 len, u32 speed_hz,
    #   u16 delay_usecs, u8 bits_per_word, u8 cs_change,
    #   u8 tx_nbits, u8 rx_nbits, u16 pad
    xfer = struct.pack('QQIIHBBBBH',
        tx_addr, rx_addr, length, SPI_SPEED,
        0, SPI_BITS, 0, 0, 0, 0)

    fcntl.ioctl(fd, SPI_IOC_MESSAGE_1, xfer)
    return bytes(rx_buf)


def build_msg(msg_type, data):
    """Build SPI message frame: [type_hi, type_lo, len_hi, len_lo, checksum, data..., pad to 8]."""
    t_hi = (msg_type >> 8) & 0xFF
    t_lo = msg_type & 0xFF
    l_hi = (len(data) >> 8) & 0xFF
    l_lo = len(data) & 0xFF
    cksum = (t_hi + t_lo + l_hi + l_lo + sum(data)) & 0xFF
    msg = bytes([t_hi, t_lo, l_hi, l_lo, cksum]) + bytes(data)
    while len(msg) % 8 != 0:
        msg += b'\x00'
    return msg


def main():
    print(f"Opening {SPI_DEV}...")
    fd = os.open(SPI_DEV, os.O_RDWR)

    # Configure SPI (mode 3, 8-bit, messaging speed)
    fcntl.ioctl(fd, SPI_IOC_WR_MODE, struct.pack('B', SPI_MODE))
    fcntl.ioctl(fd, SPI_IOC_WR_BITS, struct.pack('B', SPI_BITS))
    fcntl.ioctl(fd, SPI_IOC_WR_SPEED, struct.pack('I', SPI_SPEED))

    out = open(OUT_PATH, 'wb')
    total_nonzero = 0
    t0 = time.time()

    print(f"Dumping {PAGES} pages to {OUT_PATH}...")

    for page in range(PAGES):
        # Send memory dump command: msg_type=0x0000, data=[0x0C, page_hi, page_lo]
        p_hi = (page >> 8) & 0xFF
        p_lo = page & 0xFF
        cmd = build_msg(0x0000, [0x0C, p_hi, p_lo])

        # Send command (discard response during TX)
        spi_transfer(fd, cmd, len(cmd))

        # Wait for DSP to prepare response
        time.sleep(0.200)

        # Read response: send zeros, clock out data
        zeros = bytes(RX_SIZE)
        rx = spi_transfer(fd, zeros, RX_SIZE)

        out.write(rx)
        nz = sum(1 for b in rx if b != 0)
        total_nonzero += nz

        if page % 32 == 0:
            elapsed = time.time() - t0
            print(f"  page {page:3d}/{PAGES} ({nz:4d} non-zero bytes) [{elapsed:.1f}s]")

    out.close()
    os.close(fd)

    elapsed = time.time() - t0
    total_bytes = PAGES * RX_SIZE
    print(f"\nDone: {total_bytes} bytes written to {OUT_PATH}")
    print(f"  Non-zero bytes: {total_nonzero} / {total_bytes} ({100*total_nonzero/total_bytes:.1f}%)")
    print(f"  Elapsed: {elapsed:.1f}s ({elapsed/PAGES*1000:.0f}ms/page)")


if __name__ == '__main__':
    main()
