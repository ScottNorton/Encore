#!/usr/bin/env python3
"""ADSP-21489 .ldr file builder / patcher.

Rebuilds a DSP firmware .ldr file from components:
- Boot kernel (first 1536 bytes = 256 × 6-byte instructions)
- Block stream (everything after the boot kernel)

Can patch just the boot kernel while preserving the original block data,
or rebuild the entire file from scratch.

Usage:
  # Verify round-trip: rebuild .ldr from its own components
  python scripts/ldr_build.py dsp-img.ldr --verify

  # Replace boot kernel with reassembled version
  python scripts/ldr_build.py dsp-img.ldr --kernel kernel.bin -o dsp-img-rebuilt.ldr

  # Extract boot kernel only
  python scripts/ldr_build.py dsp-img.ldr --extract-kernel kernel.bin

  # Extract block data only (everything after boot kernel)
  python scripts/ldr_build.py dsp-img.ldr --extract-blocks blocks.bin
"""
import sys, os, argparse

BOOT_KERNEL_SIZE = 256 * 6  # 1536 bytes


def main():
    parser = argparse.ArgumentParser(description="ADSP-21489 .ldr builder / patcher")
    parser.add_argument("ldr_file", help="Original .ldr firmware file")
    parser.add_argument("--kernel", help="Replacement boot kernel binary (1536 bytes)")
    parser.add_argument("-o", "--output", help="Output .ldr file")
    parser.add_argument("--verify", action="store_true",
                        help="Verify: split and rejoin, check byte-identical")
    parser.add_argument("--extract-kernel", metavar="FILE",
                        help="Extract boot kernel to file")
    parser.add_argument("--extract-blocks", metavar="FILE",
                        help="Extract block data to file")
    parser.add_argument("--extract-kernel-raw", metavar="FILE",
                        help="Extract boot kernel with bit-reversal (SPI format)")
    args = parser.parse_args()

    # Read original
    with open(args.ldr_file, "rb") as f:
        original = f.read()

    if len(original) < BOOT_KERNEL_SIZE:
        print(f"ERROR: File too small ({len(original)} bytes, need >= {BOOT_KERNEL_SIZE})")
        sys.exit(1)

    orig_kernel = original[:BOOT_KERNEL_SIZE]
    orig_blocks = original[BOOT_KERNEL_SIZE:]

    print(f"Original: {args.ldr_file} ({len(original)} bytes)")
    print(f"  Boot kernel: {BOOT_KERNEL_SIZE} bytes (256 instructions)")
    print(f"  Block data:  {len(orig_blocks)} bytes")

    # Extract operations
    if args.extract_kernel:
        with open(args.extract_kernel, "wb") as f:
            f.write(orig_kernel)
        print(f"\nExtracted kernel → {args.extract_kernel}")

    if args.extract_kernel_raw:
        # Bit-reverse each byte (SPI format, like vendor/dsp_boot_kernel.bin)
        raw = bytes(_reverse_bits(b) for b in orig_kernel)
        with open(args.extract_kernel_raw, "wb") as f:
            f.write(raw)
        print(f"\nExtracted kernel (bit-reversed) → {args.extract_kernel_raw}")

    if args.extract_blocks:
        with open(args.extract_blocks, "wb") as f:
            f.write(orig_blocks)
        print(f"\nExtracted blocks → {args.extract_blocks}")

    # Verify mode
    if args.verify:
        rebuilt = orig_kernel + orig_blocks
        if rebuilt == original:
            print(f"\nVERIFY: PASS — split+rejoin is byte-identical ({len(original)} bytes)")
        else:
            print(f"\nVERIFY: FAIL — rebuilt differs from original!")
            # Find first difference
            for i in range(min(len(rebuilt), len(original))):
                if rebuilt[i] != original[i]:
                    print(f"  First diff at byte {i}: rebuilt={rebuilt[i]:02X} original={original[i]:02X}")
                    break
            sys.exit(1)

    # Build with replacement kernel
    if args.kernel:
        with open(args.kernel, "rb") as f:
            new_kernel = f.read()

        if len(new_kernel) != BOOT_KERNEL_SIZE:
            if len(new_kernel) < BOOT_KERNEL_SIZE:
                # Pad with zeros (NOP instructions)
                print(f"\nWARNING: Kernel is {len(new_kernel)} bytes, padding to {BOOT_KERNEL_SIZE}")
                new_kernel = new_kernel + b'\x00' * (BOOT_KERNEL_SIZE - len(new_kernel))
            else:
                print(f"\nERROR: Kernel is {len(new_kernel)} bytes, expected {BOOT_KERNEL_SIZE}")
                sys.exit(1)

        rebuilt = new_kernel + orig_blocks

        # Compare kernels
        kernel_diff = sum(1 for a, b in zip(new_kernel, orig_kernel) if a != b)
        if kernel_diff == 0:
            print(f"\nKernel: IDENTICAL to original (byte-perfect rebuild)")
        else:
            print(f"\nKernel: {kernel_diff}/{BOOT_KERNEL_SIZE} bytes differ from original")

        if args.output:
            with open(args.output, "wb") as f:
                f.write(rebuilt)
            print(f"Output: {args.output} ({len(rebuilt)} bytes)")

            # Verify total size
            if len(rebuilt) == len(original):
                print(f"Size: MATCH ({len(rebuilt)} bytes)")
            else:
                print(f"Size: MISMATCH (rebuilt={len(rebuilt)}, original={len(original)})")
        else:
            print("No -o specified, dry run only")

    if not any([args.verify, args.kernel, args.extract_kernel,
                args.extract_blocks, args.extract_kernel_raw]):
        print("\nNo operation specified. Use --verify, --kernel, --extract-kernel, etc.")
        parser.print_help()


def _reverse_bits(b):
    """Reverse the 8 bits of a byte."""
    r = 0
    for i in range(8):
        r = (r << 1) | ((b >> i) & 1)
    return r


if __name__ == "__main__":
    main()
