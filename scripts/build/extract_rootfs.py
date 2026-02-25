#!/usr/bin/env python3
"""Extract rootfs SquashFS from an Harman Kardon Invoke 83_IMAGE.

Usage:
    python3 extract_rootfs.py <stock_image> <output_squashfs>

Example:
    python3 extract_rootfs.py firmware/83_IMAGE /tmp/stock_rootfs.squashfs
"""
import struct
import sys
import os

OFFSET = 0x121E680
SQUASHFS_MAGIC = 0x73717368


def main():
    if len(sys.argv) < 3:
        print(__doc__.strip())
        sys.exit(1)

    image_path = sys.argv[1]
    output_path = sys.argv[2]

    if not os.path.isfile(image_path):
        print(f"ERROR: {image_path} not found")
        sys.exit(1)

    print(f"Reading {image_path}...")
    with open(image_path, 'rb') as f:
        f.seek(OFFSET)
        magic = f.read(4)
        print(f"Magic at offset 0x{OFFSET:X}: {magic.hex()} ({magic})")
        if struct.unpack('<I', magic)[0] != SQUASHFS_MAGIC:
            print("ERROR: Not a SquashFS magic!")
            sys.exit(1)

        # Read bytes_used from superblock (offset 40 from start)
        f.seek(OFFSET + 40)
        bytes_used = struct.unpack('<Q', f.read(8))[0]
        print(f"SquashFS bytes_used: {bytes_used:,}")

        # Read the full squashfs
        f.seek(OFFSET)
        data = f.read(bytes_used)
        print(f"Read {len(data):,} bytes")

    print(f"Writing to {output_path}...")
    with open(output_path, 'wb') as f:
        written = f.write(data)
        print(f"Wrote {written:,} bytes")

    # Verify written file
    with open(output_path, 'rb') as f:
        verify = f.read(4)
        verify_magic = struct.unpack('<I', verify)[0]
        f.seek(0, 2)
        verify_size = f.tell()

    if verify_magic != SQUASHFS_MAGIC:
        print("ERROR: Verification failed — output is not valid SquashFS!")
        sys.exit(1)

    print(f"Verified: {verify_size:,} bytes, magic OK")
    print("Done!")


if __name__ == '__main__':
    main()
