#!/bin/bash
set -e
WORK=/tmp/rebuild_initramfs
rm -rf "$WORK"
mkdir -p "$WORK"
cd "$WORK"

# Extract the deployed kexec initramfs (has full BusyBox)
gzip -dc /mnt/g/HKInvoke/firmware/initramfs_kexec.cpio.gz | cpio -idm 2>/dev/null

# Replace init with the updated version that writes evidence
cp /mnt/g/HKInvoke/firmware/initramfs_tiny/init ./init
chmod 755 ./init

# Verify init has the evidence code
echo "Evidence lines: $(grep -c boot-evidence init)"

# Rebuild
find . | cpio -o -H newc 2>/dev/null | gzip -9 > /mnt/g/HKInvoke/firmware/initramfs_kexec.cpio.gz

SIZE=$(stat -c%s /mnt/g/HKInvoke/firmware/initramfs_kexec.cpio.gz)
echo "Rebuilt initramfs_kexec.cpio.gz: $SIZE bytes"

# Verify
echo "--- Init header ---"
head -10 init
echo "--- Has busybox ---"
file bin/busybox
echo "--- Has devmem ---"
ls -la bin/devmem
echo "--- Has mount ---"
ls -la bin/mount
echo "DONE"
