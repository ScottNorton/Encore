#!/usr/bin/env python3
"""Package a new SquashFS rootfs into 83_IMAGE for the Harman Kardon Invoke.

This replaces ONLY the rootfs partition in the 83_IMAGE.
The encrypted kernel, bootloader, and all other partitions are preserved.
After writing the new rootfs data, the CRC32 in the partition table header
is updated so tftp2nand accepts the image.

If the new rootfs is larger than the stock partition, use --partition-size
to expand it. The NAND rootfs partition supports up to 84,824,064 bytes.
Trailing partitions (after rootfs) are preserved at their new offset.

Usage:
    python3 package_rootfs.py <new_rootfs.squashfs> <output_image> --stock <stock_image>

Example:
    python3 package_rootfs.py build/rootfs.squashfs firmware/83_IMAGE --stock firmware/83_IMAGE_stock
    python3 package_rootfs.py build/rootfs.squashfs firmware/83_IMAGE --stock firmware/83_IMAGE_stock --partition-size 84824064
"""
import binascii
import struct
import sys
import os
import shutil
import argparse

# Rootfs location in 83_IMAGE
ROOTFS_OFFSET = 0x121E680
SQUASHFS_MAGIC = 0x73717368

# Maximum rootfs partition size proven to work on NAND (from known-good builds)
NAND_ROOTFS_MAX = 84_824_064  # 0x50E5000 = 80.9 MB

# 83_IMAGE partition table layout:
#   Entries are 0x40 bytes each, starting at offset 0x38.
#   Entry 6 (offset 0x1B8) is the rootfs partition.
#   Within each entry:
#     +0x18: partition data size (uint32 LE)
#     +0x20: CRC32 of partition data (uint32 LE)
ROOTFS_ENTRY_OFFSET = 0x1B8
ROOTFS_SIZE_FIELD = ROOTFS_ENTRY_OFFSET + 0x18  # 0x1D0
ROOTFS_CRC_FIELD = ROOTFS_ENTRY_OFFSET + 0x20   # 0x1D8


def main():
    parser = argparse.ArgumentParser(
        description="Package a SquashFS rootfs into an Harman Kardon Invoke 83_IMAGE")
    parser.add_argument("rootfs", help="New rootfs SquashFS file")
    parser.add_argument("output", help="Output 83_IMAGE path")
    parser.add_argument("--stock", required=True,
                        help="Stock 83_IMAGE to use as base")
    parser.add_argument("--partition-size", type=int, default=0,
                        help="Override rootfs partition size in bytes "
                             f"(default: auto from header, max: {NAND_ROOTFS_MAX:,})")
    args = parser.parse_args()

    if not os.path.isfile(args.rootfs):
        print(f"ERROR: {args.rootfs} not found")
        sys.exit(1)
    if not os.path.isfile(args.stock):
        print(f"ERROR: {args.stock} not found")
        sys.exit(1)

    # Validate new rootfs
    print(f"Validating {args.rootfs}...")
    with open(args.rootfs, 'rb') as f:
        magic = struct.unpack('<I', f.read(4))[0]
        if magic != SQUASHFS_MAGIC:
            print(f"ERROR: Not a SquashFS image (magic: 0x{magic:08X})")
            sys.exit(1)
        f.seek(40)
        bytes_used = struct.unpack('<Q', f.read(8))[0]
        f.seek(0, 2)
        file_size = f.tell()

    print(f"  SquashFS magic: OK")
    print(f"  File size: {file_size:,} bytes ({file_size / 1024 / 1024:.1f} MB)")
    print(f"  bytes_used: {bytes_used:,} bytes ({bytes_used / 1024 / 1024:.1f} MB)")

    # Read stock image header info
    stock_file_size = os.path.getsize(args.stock)
    with open(args.stock, 'rb') as f:
        f.seek(ROOTFS_SIZE_FIELD)
        stock_partition_size = struct.unpack('<I', f.read(4))[0]
        f.seek(ROOTFS_CRC_FIELD)
        stock_crc = struct.unpack('<I', f.read(4))[0]
        f.seek(ROOTFS_OFFSET)
        stock_magic = struct.unpack('<I', f.read(4))[0]
        if stock_magic != SQUASHFS_MAGIC:
            print(f"ERROR: No SquashFS at offset 0x{ROOTFS_OFFSET:X} in stock image!")
            sys.exit(1)

    print(f"\nStock image: {args.stock}")
    print(f"  Size: {stock_file_size:,} bytes")
    print(f"  Partition size from header: {stock_partition_size:,} bytes ({stock_partition_size / 1024 / 1024:.1f} MB)")
    print(f"  Stock CRC32: 0x{stock_crc:08X}")

    # Determine partition size to use
    if args.partition_size:
        partition_size = args.partition_size
        if partition_size > NAND_ROOTFS_MAX:
            print(f"ERROR: Requested partition size {partition_size:,} exceeds NAND max {NAND_ROOTFS_MAX:,}")
            sys.exit(1)
    elif file_size > stock_partition_size:
        # Auto-expand: rootfs doesn't fit in stock partition, use NAND max
        print(f"\n  Rootfs ({file_size:,}) exceeds stock partition ({stock_partition_size:,})")
        print(f"  Auto-expanding to NAND max: {NAND_ROOTFS_MAX:,} bytes ({NAND_ROOTFS_MAX / 1024 / 1024:.1f} MB)")
        partition_size = NAND_ROOTFS_MAX
    else:
        partition_size = stock_partition_size

    if file_size > partition_size:
        print(f"ERROR: Rootfs too large! ({file_size:,} > {partition_size:,})")
        sys.exit(1)

    resizing = partition_size != stock_partition_size

    # Read trailing data (partitions after rootfs) from stock before modifying
    stock_rootfs_end = ROOTFS_OFFSET + stock_partition_size
    trailing_data = b''
    if stock_rootfs_end < stock_file_size:
        with open(args.stock, 'rb') as f:
            f.seek(stock_rootfs_end)
            trailing_data = f.read()
        print(f"  Trailing partitions: {len(trailing_data):,} bytes (preserved)")

    # Copy stock image to output
    print(f"\nCopying stock image to {args.output}...")
    shutil.copy2(args.stock, args.output)

    # Update partition size in header if resizing
    if resizing:
        print(f"  Updating partition size: {stock_partition_size:,} -> {partition_size:,}")
        with open(args.output, 'r+b') as f:
            f.seek(ROOTFS_SIZE_FIELD)
            f.write(struct.pack('<I', partition_size))

    # Read new rootfs and pad to partition size
    with open(args.rootfs, 'rb') as f:
        new_data = f.read()

    padding_needed = partition_size - len(new_data)
    if padding_needed > 0:
        print(f"  Padding with {padding_needed:,} zero bytes to fill partition")
        new_data += b'\x00' * padding_needed

    # Write new rootfs into image
    print(f"\nWriting new rootfs at offset 0x{ROOTFS_OFFSET:X}...")
    with open(args.output, 'r+b') as f:
        f.seek(ROOTFS_OFFSET)
        f.write(new_data)

    # Write trailing partitions at new offset
    if trailing_data:
        new_rootfs_end = ROOTFS_OFFSET + partition_size
        with open(args.output, 'r+b') as f:
            f.seek(new_rootfs_end)
            f.write(trailing_data)
        # Truncate to exact size (remove any leftover from stock if we shrank)
        expected_size = new_rootfs_end + len(trailing_data)
        with open(args.output, 'r+b') as f:
            f.truncate(expected_size)

    # Compute CRC32 of the full partition data (padded)
    new_crc = binascii.crc32(new_data) & 0xFFFFFFFF
    print(f"  New CRC32: 0x{new_crc:08X}")

    # Update CRC in header
    print(f"  Writing CRC to header offset 0x{ROOTFS_CRC_FIELD:X}...")
    with open(args.output, 'r+b') as f:
        f.seek(ROOTFS_CRC_FIELD)
        f.write(struct.pack('<I', new_crc))

    # Verify
    with open(args.output, 'rb') as f:
        f.seek(ROOTFS_CRC_FIELD)
        verify_crc = struct.unpack('<I', f.read(4))[0]
        f.seek(ROOTFS_SIZE_FIELD)
        verify_part_size = struct.unpack('<I', f.read(4))[0]
        f.seek(ROOTFS_OFFSET)
        verify_magic = struct.unpack('<I', f.read(4))[0]
        f.seek(ROOTFS_OFFSET + 40)
        verify_bytes = struct.unpack('<Q', f.read(8))[0]

    print(f"\nVerification:")
    print(f"  Header CRC: 0x{verify_crc:08X} {'OK' if verify_crc == new_crc else 'MISMATCH!'}")
    print(f"  Partition size: {verify_part_size:,} {'OK' if verify_part_size == partition_size else 'MISMATCH!'}")
    print(f"  SquashFS magic: 0x{verify_magic:08X} {'OK' if verify_magic == SQUASHFS_MAGIC else 'FAILED'}")
    print(f"  SquashFS bytes_used: {verify_bytes:,}")

    final_size = os.path.getsize(args.output)
    print(f"\nOutput: {args.output}")
    print(f"  Size: {final_size:,} bytes")
    print(f"\nDone! Flash with: tftp2nand -d {final_size} 0x7000000")


if __name__ == '__main__':
    main()
