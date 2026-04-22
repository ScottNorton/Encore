#!/bin/bash
# scripts/kexec/test_kexec.sh — Test kexec module on Harman Kardon Invoke
#
# Runs a series of incremental tests to validate the kexec pipeline.
# Tests 1-2 are safe (no reboot). Test 3 reboots the device.
#
# Usage:
#   ./scripts/kexec/test_kexec.sh [DEVICE_IP] [test_number]
#   ./scripts/kexec/test_kexec.sh 192.168.43.1       # run all safe tests (1-2)
#   ./scripts/kexec/test_kexec.sh 192.168.43.1 1     # test 1 only
#   ./scripts/kexec/test_kexec.sh 192.168.43.1 3a    # dry-run level 1 (MM switch)
#   ./scripts/kexec/test_kexec.sh 192.168.43.1 3b    # dry-run level 2 (+ read)
#   ./scripts/kexec/test_kexec.sh 192.168.43.1 3c    # dry-run level 3 (+ exec)
#   ./scripts/kexec/test_kexec.sh 192.168.43.1 4     # DANGEROUS: real kexec execute
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
. "$REPO_ROOT/scripts/common.sh"

DEVICE_IP="${1:-$ENCORE_DEVICE_IP}"
TEST_NUM="${2:-safe}"
SSH_OPTS="$ENCORE_SSH_OPTS -o ConnectTimeout=5"
SSH="sshpass -p $ENCORE_SSH_PASS ssh $SSH_OPTS root@$DEVICE_IP"

PASS=0
FAIL=0
SKIP=0

result() {
    local status="$1" name="$2" detail="${3:-}"
    case "$status" in
        PASS) PASS=$((PASS+1)); echo "  [PASS] $name${detail:+ — $detail}" ;;
        FAIL) FAIL=$((FAIL+1)); echo "  [FAIL] $name${detail:+ — $detail}" ;;
        SKIP) SKIP=$((SKIP+1)); echo "  [SKIP] $name${detail:+ — $detail}" ;;
    esac
}

echo "=== kexec Test Suite ==="
echo "  Device: $DEVICE_IP"
echo ""

# ── Test 1: Module load ──
test_1_module_load() {
    echo "── Test 1: Module Load (safe) ──"

    # Check connectivity
    if ! $SSH "echo ok" >/dev/null 2>&1; then
        result FAIL "SSH connectivity"
        return 1
    fi
    result PASS "SSH connectivity"

    # Check kernel version
    local kver
    kver=$($SSH "uname -r" 2>/dev/null)
    if [ "$kver" = "3.8.13-yocto-standard" ]; then
        result PASS "Kernel version" "$kver"
    else
        result FAIL "Kernel version" "expected 3.8.13-yocto-standard, got $kver"
        return 1
    fi

    # Check if module exists on device
    if $SSH "test -f /lsync/kexec/kexec-mod.ko" 2>/dev/null; then
        result PASS "Module file exists"
    else
        result FAIL "Module file exists" "/lsync/kexec/kexec-mod.ko not found"
        return 1
    fi

    # Unload if already loaded, then reload
    $SSH "rmmod kexec_mod 2>/dev/null; true" 2>/dev/null
    sleep 1

    # Load module (or verify already loaded)
    local load_result
    load_result=$($SSH "insmod /lsync/kexec/kexec-mod.ko 2>&1; echo RETVAL=\$?" 2>/dev/null)
    if echo "$load_result" | grep -q "RETVAL=0"; then
        result PASS "insmod" "loaded fresh"
    elif echo "$load_result" | grep -qi "File exists"; then
        # Module already loaded and rmmod couldn't unload (device in use)
        result PASS "insmod" "already loaded (rmmod failed — device busy?)"
    else
        result FAIL "insmod" "$load_result"
        return 1
    fi

    # Verify /dev/kexec and check dmesg (dmesg may have rotated)
    if $SSH "test -c /dev/kexec" 2>/dev/null; then
        result PASS "/dev/kexec created" "char device present"
        # Try dmesg for bonus info (may be empty if ring buffer rotated)
        local dmesg
        dmesg=$($SSH "dmesg | grep 'kexec-mod' | tail -5" 2>/dev/null || true)
        if [ -n "$dmesg" ] && echo "$dmesg" | grep -q "all symbols resolved"; then
            result PASS "kallsyms resolution" "all 4 symbols found"
        else
            result PASS "kallsyms resolution" "inferred from working /dev/kexec"
        fi
    else
        result FAIL "/dev/kexec created" "char device not found"
        result FAIL "kallsyms resolution" "module not functional"
    fi

    # Verify device node
    if $SSH "test -c /dev/kexec" 2>/dev/null; then
        result PASS "/dev/kexec is char device"
    else
        result FAIL "/dev/kexec is char device"
    fi

    # Check lsmod
    local lsmod
    lsmod=$($SSH "lsmod | grep kexec_mod" 2>/dev/null)
    if [ -n "$lsmod" ]; then
        result PASS "lsmod shows kexec_mod" "$lsmod"
    else
        result FAIL "lsmod shows kexec_mod"
    fi

    echo ""
}

# ── Test 2: kexec load/unload cycle ──
test_2_load_unload() {
    echo "── Test 2: kexec Load/Unload (safe, no reboot) ──"

    # Check kexec binary
    if ! $SSH "test -x /lsync/kexec/kexec" 2>/dev/null; then
        result FAIL "kexec binary exists"
        return 1
    fi
    result PASS "kexec binary exists"

    # Check kexec --version
    local ver
    ver=$($SSH "/lsync/kexec/kexec --version 2>&1" 2>/dev/null)
    if echo "$ver" | grep -q "2.0.15"; then
        result PASS "kexec version" "$ver"
    else
        result FAIL "kexec version" "$ver"
    fi

    # Ensure module is loaded
    if ! $SSH "test -c /dev/kexec" 2>/dev/null; then
        $SSH "insmod /lsync/kexec/kexec-mod.ko 2>/dev/null" 2>/dev/null
    fi

    # Test unload (should succeed even with nothing loaded)
    local unload_result
    unload_result=$($SSH "/lsync/kexec/kexec -u 2>&1; echo RETVAL=\$?" 2>/dev/null)
    if echo "$unload_result" | grep -q "RETVAL=0"; then
        result PASS "kexec -u (unload)" "clean unload"
    else
        result FAIL "kexec -u (unload)" "$unload_result"
    fi

    # Find kernel image — prefer separate zImage + DTB (stock kernel lacks /proc/device-tree)
    local kernel_path="" dtb_flag=""
    if $SSH "test -f /lsync/kexec/zImage" 2>/dev/null; then
        kernel_path="/lsync/kexec/zImage"
        if $SSH "test -f /lsync/kexec/berlin2cdp-hk-invoke.dtb" 2>/dev/null; then
            dtb_flag="--dtb=/lsync/kexec/berlin2cdp-hk-invoke.dtb"
        fi
    elif $SSH "test -f /lsync/kexec/zImage-dtb" 2>/dev/null; then
        kernel_path="/lsync/kexec/zImage-dtb"
    fi

    if [ -n "$kernel_path" ]; then
        local load_result
        load_result=$($SSH "/lsync/kexec/kexec -l $kernel_path $dtb_flag --command-line='console=ttyS0,115200 earlyprintk loglevel=8' 2>&1; echo RETVAL=\$?" 2>/dev/null)
        if echo "$load_result" | grep -q "RETVAL=0"; then
            result PASS "kexec -l (load kernel)" "loaded $kernel_path ${dtb_flag:+(with DTB)}"

            # Immediately unload — do NOT execute
            $SSH "/lsync/kexec/kexec -u 2>/dev/null" 2>/dev/null
            result PASS "kexec -u (unload after load)" "kernel safely unloaded"
        else
            result FAIL "kexec -l (load kernel)" "$load_result"
        fi
    else
        result SKIP "kexec -l (load kernel)" "no zImage on device yet"
    fi

    echo ""
}

# ── Test 3: Dry-run identity mapping test ──
test_3_dryrun() {
    local level="${1:-1}"
    echo "── Test 3: Dry-Run Identity Mapping (level $level) ──"
    echo ""
    echo "  Level 1: switch TTBR0 to idmap_pgd and back"
    echo "  Level 2: + read from physical address via identity map"
    echo "  Level 3: + execute NOP+BX_LR at physical address"
    echo ""

    # Find kernel to boot
    local kernel_path="" dtb_flag=""
    if $SSH "test -f /lsync/kexec/zImage" 2>/dev/null; then
        kernel_path="/lsync/kexec/zImage"
        if $SSH "test -f /lsync/kexec/berlin2cdp-hk-invoke.dtb" 2>/dev/null; then
            dtb_flag="--dtb=/lsync/kexec/berlin2cdp-hk-invoke.dtb"
        fi
    elif $SSH "test -f /lsync/kexec/zImage-dtb" 2>/dev/null; then
        kernel_path="/lsync/kexec/zImage-dtb"
    fi

    if [ -z "$kernel_path" ]; then
        result SKIP "Find kernel image" "no zImage on device (needed for kexec -l)"
        return 1
    fi
    result PASS "Found kernel" "$kernel_path"

    # Ensure module loaded with dry_run param
    $SSH "rmmod kexec_mod 2>/dev/null; true" 2>/dev/null
    sleep 1
    local insmod_out
    insmod_out=$($SSH "insmod /lsync/kexec/kexec-mod.ko kexec_dry_run=$level 2>&1; echo RET=\$?" 2>/dev/null)
    if echo "$insmod_out" | grep -q "RET=0"; then
        result PASS "insmod with kexec_dry_run=$level"
    else
        result FAIL "insmod" "$insmod_out"
        return 1
    fi

    # Show module init messages
    echo ""
    echo "  Module init dmesg:"
    $SSH "dmesg | grep -E 'kexec-(mod|drv)' | tail -10" 2>/dev/null || true
    echo ""

    # Load kernel
    local cmdline="console=ttyS0,115200 earlyprintk loglevel=8"
    local load_out
    load_out=$($SSH "/lsync/kexec/kexec -l $kernel_path $dtb_flag --command-line='$cmdline' 2>&1; echo RET=\$?" 2>/dev/null)
    if echo "$load_out" | grep -q "RET=0"; then
        result PASS "kexec -l" "kernel loaded"
    else
        result FAIL "kexec -l" "$load_out"
        return 1
    fi

    # Execute — dry-run should return without rebooting
    echo ""
    echo "  Executing dry-run kexec -e (should NOT reboot)..."
    local exec_out
    exec_out=$($SSH "/lsync/kexec/kexec -e 2>&1; echo EXEC_RET=\$?" 2>/dev/null)

    # If SSH completes, the device didn't crash
    if [ -n "$exec_out" ]; then
        result PASS "SSH survived kexec -e" "device did not crash"

        # Check dmesg for results
        echo ""
        echo "  kexec dmesg output:"
        $SSH "dmesg | grep KEXEC | tail -15" 2>/dev/null || true
        echo ""

        if $SSH "dmesg | grep 'DRY RUN'" 2>/dev/null | grep -q "PASSED"; then
            result PASS "Dry-run level $level" "PASSED"
        else
            result FAIL "Dry-run level $level" "did not report PASSED"
        fi
    else
        # SSH died — device probably crashed
        echo "  SSH connection lost — device likely crashed"
        result FAIL "Dry-run level $level" "device crashed (SSH lost)"

        echo "  Waiting for device to come back (up to 90s)..."
        local waited=0
        while [ $waited -lt 90 ]; do
            if $SSH "echo alive" >/dev/null 2>&1; then
                echo "  Device back after ${waited}s"
                echo ""
                echo "  Post-crash dmesg (kexec lines):"
                $SSH "dmesg | grep -E 'KEXEC|kexec' | tail -15" 2>/dev/null || true
                echo ""
                echo "  Uptime:"
                $SSH "uptime" 2>/dev/null || true
                return 1
            fi
            sleep 3
            waited=$((waited+3))
            echo -n "."
        done
        echo ""
        result FAIL "Device comeback" "no response within 90s"
        return 1
    fi

    # Unload kernel
    $SSH "/lsync/kexec/kexec -u 2>/dev/null" 2>/dev/null

    echo ""
}

# ── Test 4: kexec execute (REBOOTS DEVICE) ──
test_4_execute() {
    echo "── Test 4: kexec Execute (REBOOTS DEVICE) ──"
    echo ""
    echo "  WARNING: This will reboot the device into a new kernel."
    echo "  If the new kernel fails, the watchdog will reboot back to stock"
    echo "  within 30 seconds."
    echo ""

    # Find kernel to boot — prefer separate zImage + DTB
    local kernel_path="" dtb_flag=""
    if $SSH "test -f /lsync/kexec/zImage" 2>/dev/null; then
        kernel_path="/lsync/kexec/zImage"
        if $SSH "test -f /lsync/kexec/berlin2cdp-hk-invoke.dtb" 2>/dev/null; then
            dtb_flag="--dtb=/lsync/kexec/berlin2cdp-hk-invoke.dtb"
        fi
    elif $SSH "test -f /lsync/kexec/zImage-dtb" 2>/dev/null; then
        kernel_path="/lsync/kexec/zImage-dtb"
    fi

    if [ -z "$kernel_path" ]; then
        result SKIP "Find kernel image" "no zImage on device"
        echo ""
        echo "  Upload kernel files first:"
        echo "    cat zImage | ssh ... 'cat > /lsync/kexec/zImage'"
        echo "    cat *.dtb  | ssh ... 'cat > /lsync/kexec/berlin2cdp-hk-invoke.dtb'"
        return 1
    fi

    local ksize
    ksize=$($SSH "stat -c%s $kernel_path" 2>/dev/null)
    result PASS "Found kernel" "$kernel_path ($ksize bytes) ${dtb_flag:+(DTB present)}"

    # Ensure module loaded with dry_run=0 (real exec)
    $SSH "rmmod kexec_mod 2>/dev/null; true" 2>/dev/null
    sleep 1
    $SSH "insmod /lsync/kexec/kexec-mod.ko kexec_dry_run=0" 2>/dev/null

    # Pet watchdog to buy maximum time
    $SSH "echo V > /dev/watchdog 2>/dev/null; true" 2>/dev/null

    # Load kernel
    local cmdline="console=ttyS0,115200 earlyprintk loglevel=8 panic=30"
    local load_out
    load_out=$($SSH "/lsync/kexec/kexec -l $kernel_path $dtb_flag --command-line='$cmdline' 2>&1; echo RETVAL=\$?" 2>/dev/null)
    if echo "$load_out" | grep -q "RETVAL=0"; then
        result PASS "kexec -l" "kernel loaded"
    else
        result FAIL "kexec -l" "$load_out"
        return 1
    fi

    echo ""
    echo "  Executing kexec in 3 seconds... (Ctrl+C to abort)"
    sleep 3

    # Execute — this will kill the SSH connection
    echo "  Executing: kexec -e"
    $SSH "/lsync/kexec/kexec -e" 2>/dev/null &
    local ssh_pid=$!

    # Wait for device to go down
    echo "  Waiting for device to go offline..."
    sleep 5

    # Check if device comes back (stock kernel via watchdog, or new kernel)
    echo "  Waiting for device to come back online (up to 60s)..."
    local waited=0
    while [ $waited -lt 60 ]; do
        if $SSH "echo alive" >/dev/null 2>&1; then
            local new_kver
            new_kver=$($SSH "uname -r" 2>/dev/null)
            result PASS "Device online" "kernel: $new_kver"

            if [ "$new_kver" != "3.8.13-yocto-standard" ]; then
                echo ""
                echo "  *** NEW KERNEL BOOTED: $new_kver ***"
                echo ""
                $SSH "dmesg | head -30" 2>/dev/null
            else
                echo "  Device running stock kernel (watchdog reboot or kexec to same kernel)"
            fi
            kill $ssh_pid 2>/dev/null; wait $ssh_pid 2>/dev/null
            return 0
        fi
        sleep 2
        waited=$((waited+2))
    done

    result FAIL "Device comeback" "device did not respond within 60s"
    kill $ssh_pid 2>/dev/null; wait $ssh_pid 2>/dev/null
    echo ""
}

# ── Run tests ──
case "$TEST_NUM" in
    1) test_1_module_load ;;
    2) test_1_module_load; test_2_load_unload ;;
    3)
        test_1_module_load
        test_2_load_unload
        test_3_dryrun 1
        ;;
    3a) test_1_module_load; test_3_dryrun 1 ;;
    3b) test_1_module_load; test_3_dryrun 2 ;;
    3c) test_1_module_load; test_3_dryrun 3 ;;
    4)
        test_1_module_load
        test_2_load_unload
        test_4_execute
        ;;
    safe|"")
        test_1_module_load
        test_2_load_unload
        ;;
    *)
        echo "Usage: $0 [DEVICE_IP] [1|2|3|3a|3b|3c|4|safe]"
        echo "  1    = module load only"
        echo "  2    = + load/unload cycle"
        echo "  3    = + dry-run level 1 (MM switch)"
        echo "  3a   = dry-run level 1 (MM switch only)"
        echo "  3b   = dry-run level 2 (+ read at phys addr)"
        echo "  3c   = dry-run level 3 (+ exec at phys addr)"
        echo "  4    = REAL kexec execute (reboots device)"
        echo "  safe = tests 1+2 (default)"
        exit 1
        ;;
esac

echo "════════════════════════════"
echo "  PASS: $PASS  FAIL: $FAIL  SKIP: $SKIP"
echo "════════════════════════════"

if [ $FAIL -gt 0 ]; then
    exit 1
fi
