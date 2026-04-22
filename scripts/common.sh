# shellcheck shell=bash
# scripts/common.sh — shared configuration for host-side device helpers.
#
# Source this from any script that talks to the speaker over SSH:
#
#     SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
#     . "$SCRIPT_DIR/../common.sh"    # from scripts/device/*.sh
#     . "$SCRIPT_DIR/../common.sh"    # from scripts/kexec/*.sh
#
# After sourcing you have:
#
#     $ENCORE_DEVICE_IP   — target speaker (default 192.168.43.1, the AP)
#     $ENCORE_SSH_PASS    — dropbear password (default ridiculous)
#     $ENCORE_SSH_OPTS    — ssh options for the legacy ssh-rsa algorithms
#     $ENCORE_SSH         — prebuilt `sshpass ... ssh root@$ENCORE_DEVICE_IP`
#     $ENCORE_SCP         — prebuilt `sshpass ... scp` (caller appends paths)
#
# To override the device IP per machine, drop a .encore-env file at the
# repo root (gitignored, see .encore-env.example) or export
# ENCORE_DEVICE_IP in your shell before running a script. Most scripts
# also accept a positional IP as `$1` for one-off overrides.

# Load optional user-local overrides from the repo root.
_encore_repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ -f "$_encore_repo_root/.encore-env" ]; then
    # shellcheck disable=SC1091
    . "$_encore_repo_root/.encore-env"
fi
unset _encore_repo_root

# Defaults. The AP IP is what every speaker serves on first boot before
# it's joined your WiFi. After it's on your LAN, override via .encore-env
# (e.g. ENCORE_DEVICE_IP=encore.local or your DHCP-assigned address).
: "${ENCORE_DEVICE_IP:=192.168.43.1}"
: "${ENCORE_SSH_PASS:=ridiculous}"

ENCORE_SSH_OPTS="-o HostKeyAlgorithms=+ssh-rsa -o PubkeyAcceptedKeyTypes=+ssh-rsa -o StrictHostKeyChecking=no"
ENCORE_SSH="sshpass -p $ENCORE_SSH_PASS ssh $ENCORE_SSH_OPTS root@$ENCORE_DEVICE_IP"
ENCORE_SCP="sshpass -p $ENCORE_SSH_PASS scp $ENCORE_SSH_OPTS"

export ENCORE_DEVICE_IP ENCORE_SSH_PASS ENCORE_SSH_OPTS ENCORE_SSH ENCORE_SCP
