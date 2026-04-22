#!/usr/bin/env python3
"""Bluetooth discovery tool for Harman Kardon Invoke.

Uses raw kernel sockets (AF_BLUETOOTH) to interact with the 88W8887 BT chip.
No BlueZ required — talks directly to the kernel's HCI management API.

Usage:
    python3 bt_discover.py load       # Load bt8xxx.ko module
    python3 bt_discover.py info       # Read adapter info (BD_ADDR, features)
    python3 bt_discover.py up         # Power on + make discoverable + connectable
    python3 bt_discover.py down       # Power off adapter
    python3 bt_discover.py scan       # Scan for nearby BT devices
    python3 bt_discover.py pair       # Enable pairing mode (Just Works)
    python3 bt_discover.py monitor    # Monitor all mgmt events
    python3 bt_discover.py hci CMD    # Send raw HCI command (hex)
"""

import fcntl
import os
import socket
import struct
import subprocess
import sys
import time

# ── Constants ──

AF_BLUETOOTH = 31
BTPROTO_HCI = 1
HCI_CHANNEL_CONTROL = 3  # Management API
HCI_DEV_NONE = 0xFFFF

BT_MODULE = "/lib/modules/3.8.13-yocto-standard/kernel/arch/arm/mach-berlin/modules/bt_sd8887/bt8xxx.ko"
WIFI_MAC_PATH = "/sys/class/net/wlan0/address"
HCI_SYSFS = "/sys/class/bluetooth/hci0"

# Management API opcodes
MGMT_READ_VERSION   = 0x0001
MGMT_READ_COMMANDS  = 0x0002
MGMT_READ_INDEX     = 0x0003
MGMT_READ_INFO      = 0x0004
MGMT_SET_POWERED    = 0x0005
MGMT_SET_DISCOVERABLE = 0x0006
MGMT_SET_CONNECTABLE = 0x0007
MGMT_SET_PAIRABLE   = 0x0009
MGMT_SET_LINK_SECURITY = 0x000A
MGMT_SET_SSP        = 0x000B
MGMT_SET_LE         = 0x000D
MGMT_LOAD_LINK_KEYS = 0x0012
MGMT_SET_IO_CAP     = 0x0018
MGMT_SET_LOCAL_NAME = 0x000F
MGMT_START_DISCOVERY = 0x0023
MGMT_STOP_DISCOVERY  = 0x0024
MGMT_CONFIRM_REPLY   = 0x001C
MGMT_PIN_CODE_REPLY  = 0x001E

# Event opcodes
MGMT_EV_CMD_COMPLETE = 0x0001
MGMT_EV_CMD_STATUS   = 0x0002
MGMT_EV_INDEX_ADDED  = 0x0004
MGMT_EV_INDEX_REMOVED = 0x0005
MGMT_EV_NEW_SETTINGS = 0x0006
MGMT_EV_DISCOVERING  = 0x0013
MGMT_EV_DEVICE_FOUND = 0x0012
MGMT_EV_CONNECTING   = 0x000F
MGMT_EV_CONNECTED    = 0x000B
MGMT_EV_DISCONNECTED = 0x000C
MGMT_EV_PIN_CODE_REQ = 0x0010
MGMT_EV_USER_CONFIRM = 0x000F
MGMT_EV_AUTH_FAILED  = 0x0011
MGMT_EV_NEW_LINK_KEY = 0x0009
MGMT_EV_DEVICE_UNPAIRED = 0x001B

# Event name lookup
MGMT_EVENT_NAMES = {
    0x0001: "CMD_COMPLETE",
    0x0002: "CMD_STATUS",
    0x0003: "CONTROLLER_ERROR",
    0x0004: "INDEX_ADDED",
    0x0005: "INDEX_REMOVED",
    0x0006: "NEW_SETTINGS",
    0x0007: "CLASS_OF_DEV_CHANGED",
    0x0008: "LOCAL_NAME_CHANGED",
    0x0009: "NEW_LINK_KEY",
    0x000A: "NEW_LONG_TERM_KEY",
    0x000B: "DEVICE_CONNECTED",
    0x000C: "DEVICE_DISCONNECTED",
    0x000D: "CONNECT_FAILED",
    0x000E: "PIN_CODE_REQUEST",
    0x000F: "USER_CONFIRM_REQUEST",
    0x0010: "USER_PASSKEY_REQUEST",
    0x0011: "AUTH_FAILED",
    0x0012: "DEVICE_FOUND",
    0x0013: "DISCOVERING",
    0x0014: "DEVICE_BLOCKED",
    0x0015: "DEVICE_UNBLOCKED",
    0x0016: "DEVICE_UNPAIRED",
    0x0017: "PASSKEY_NOTIFY",
    0x0018: "NEW_IRK",
    0x0019: "NEW_CSRK",
    0x001A: "DEVICE_ADDED",
    0x001B: "DEVICE_REMOVED",
    0x001C: "NEW_CONN_PARAM",
    0x001D: "UNCONF_INDEX_ADDED",
    0x001E: "UNCONF_INDEX_REMOVED",
    0x001F: "NEW_CONFIG_OPTIONS",
}


class MgmtSocket:
    """Raw HCI Management API socket."""

    def __init__(self):
        self.sock = socket.socket(AF_BLUETOOTH, socket.SOCK_RAW, BTPROTO_HCI)
        # Bind to HCI_CHANNEL_CONTROL
        # struct sockaddr_hci { sa_family, hci_dev (u16), hci_channel (u16) }
        addr = struct.pack("<HHH", AF_BLUETOOTH, HCI_DEV_NONE, HCI_CHANNEL_CONTROL)
        self.sock.bind(addr)
        self.sock.settimeout(5.0)

    def close(self):
        self.sock.close()

    def send_cmd(self, opcode, index=0x0000, data=b''):
        """Send a management command. Header: opcode(2) + index(2) + len(2) + data."""
        hdr = struct.pack("<HHH", opcode, index, len(data))
        self.sock.send(hdr + data)

    def recv_event(self, timeout=5.0):
        """Receive a management event. Returns (opcode, index, data)."""
        self.sock.settimeout(timeout)
        try:
            raw = self.sock.recv(4096)
        except socket.timeout:
            return None
        if len(raw) < 6:
            return None
        opcode, index, plen = struct.unpack("<HHH", raw[:6])
        data = raw[6:6+plen]
        return (opcode, index, data)

    def wait_cmd_complete(self, expected_opcode, timeout=5.0):
        """Wait for CMD_COMPLETE for a specific opcode."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            evt = self.recv_event(timeout=deadline - time.time())
            if evt is None:
                continue
            opcode, index, data = evt
            if opcode == MGMT_EV_CMD_COMPLETE and len(data) >= 3:
                cmd_opcode, status = struct.unpack("<HB", data[:3])
                if cmd_opcode == expected_opcode:
                    return (status, data[3:])
            # Print other events while waiting
            name = MGMT_EVENT_NAMES.get(opcode, f"0x{opcode:04x}")
            print(f"  (event: {name}, idx={index}, {len(data)} bytes)")
        return None

    # ── High-level operations ──

    def read_version(self):
        """Read management API version."""
        self.send_cmd(MGMT_READ_VERSION, index=HCI_DEV_NONE & 0xFFFF)
        result = self.wait_cmd_complete(MGMT_READ_VERSION)
        if result and result[0] == 0 and len(result[1]) >= 3:
            ver, rev = struct.unpack("<BH", result[1][:3])
            return (ver, rev)
        return None

    def read_info(self, index=0):
        """Read adapter info. Returns dict with address, name, settings, etc."""
        self.send_cmd(MGMT_READ_INFO, index=index)
        result = self.wait_cmd_complete(MGMT_READ_INFO)
        if result is None or result[0] != 0:
            return None
        data = result[1]
        if len(data) < 280:
            return {"raw": data.hex(' ')}

        addr = data[0:6]
        bt_ver = data[6]
        manufacturer = struct.unpack("<H", data[7:9])[0]
        supported = struct.unpack("<I", data[9:13])[0]
        current = struct.unpack("<I", data[13:17])[0]
        dev_class = data[17:20]
        name = data[20:269].split(b'\x00')[0].decode('utf-8', errors='replace')
        short_name = data[269:280].split(b'\x00')[0].decode('utf-8', errors='replace')

        addr_str = ':'.join(f'{b:02X}' for b in reversed(addr))
        return {
            'address': addr_str,
            'bt_version': bt_ver,
            'manufacturer': manufacturer,
            'supported_settings': supported,
            'current_settings': current,
            'class': dev_class.hex(),
            'name': name,
            'short_name': short_name,
        }

    def set_powered(self, on, index=0):
        self.send_cmd(MGMT_SET_POWERED, index=index, data=bytes([1 if on else 0]))
        return self.wait_cmd_complete(MGMT_SET_POWERED)

    def set_discoverable(self, on, timeout_sec=0, index=0):
        data = struct.pack("<BH", 1 if on else 0, timeout_sec)
        self.send_cmd(MGMT_SET_DISCOVERABLE, index=index, data=data)
        return self.wait_cmd_complete(MGMT_SET_DISCOVERABLE)

    def set_connectable(self, on, index=0):
        self.send_cmd(MGMT_SET_CONNECTABLE, index=index, data=bytes([1 if on else 0]))
        return self.wait_cmd_complete(MGMT_SET_CONNECTABLE)

    def set_pairable(self, on, index=0):
        self.send_cmd(MGMT_SET_PAIRABLE, index=index, data=bytes([1 if on else 0]))
        return self.wait_cmd_complete(MGMT_SET_PAIRABLE)

    def set_ssp(self, on, index=0):
        self.send_cmd(MGMT_SET_SSP, index=index, data=bytes([1 if on else 0]))
        return self.wait_cmd_complete(MGMT_SET_SSP)

    def set_io_capability(self, cap=0x03, index=0):
        """Set IO capability. 0x03 = NoInputNoOutput (Just Works pairing)."""
        self.send_cmd(MGMT_SET_IO_CAP, index=index, data=bytes([cap]))
        return self.wait_cmd_complete(MGMT_SET_IO_CAP)

    def set_local_name(self, name, short_name="", index=0):
        name_bytes = name.encode('utf-8')[:248].ljust(249, b'\x00')
        short_bytes = short_name.encode('utf-8')[:10].ljust(11, b'\x00')
        self.send_cmd(MGMT_SET_LOCAL_NAME, index=index, data=name_bytes + short_bytes)
        return self.wait_cmd_complete(MGMT_SET_LOCAL_NAME)

    def load_link_keys(self, keys=None, index=0):
        """Load link keys. Empty list clears all."""
        if keys is None:
            keys = []
        # debug_keys(1) + key_count(2) + keys
        data = struct.pack("<BH", 0, len(keys))
        self.send_cmd(MGMT_LOAD_LINK_KEYS, index=index, data=data)
        return self.wait_cmd_complete(MGMT_LOAD_LINK_KEYS)

    def start_discovery(self, br_edr=True, le=False, index=0):
        """Start device discovery. type: 1=BR/EDR, 6=BR/EDR+LE, 7=all."""
        addr_type = 0
        if br_edr:
            addr_type |= 1
        if le:
            addr_type |= 6
        self.send_cmd(MGMT_START_DISCOVERY, index=index, data=bytes([addr_type]))
        return self.wait_cmd_complete(MGMT_START_DISCOVERY)

    def stop_discovery(self, index=0):
        self.send_cmd(MGMT_STOP_DISCOVERY, index=index, data=bytes([1]))  # BR/EDR
        return self.wait_cmd_complete(MGMT_STOP_DISCOVERY)

    def confirm_reply(self, addr, addr_type=0x00, index=0):
        """Auto-confirm pairing (Just Works)."""
        data = bytes(addr) + bytes([addr_type])
        self.send_cmd(MGMT_CONFIRM_REPLY, index=index, data=data)
        return self.wait_cmd_complete(MGMT_CONFIRM_REPLY)


def derive_bt_mac(wifi_mac):
    """Derive BT MAC by incrementing last byte of WiFi MAC."""
    parts = wifi_mac.strip().split(':')
    if len(parts) != 6:
        raise ValueError(f"Invalid MAC: {wifi_mac}")
    b = [int(p, 16) for p in parts]
    b[5] = (b[5] + 1) & 0xFF
    return ':'.join(f'{x:02X}' for x in b)


def load_bt_module():
    """Load bt8xxx.ko with derived BT MAC address."""
    # Check if already loaded
    with open('/proc/modules') as f:
        if 'bt8xxx' in f.read():
            print("bt8xxx already loaded")
            return True

    # Derive BT MAC from WiFi MAC
    with open(WIFI_MAC_PATH) as f:
        wifi_mac = f.read().strip()
    bt_mac = derive_bt_mac(wifi_mac)
    print(f"WiFi MAC: {wifi_mac}")
    print(f"BT MAC:   {bt_mac}")

    # Load module
    print(f"Loading {BT_MODULE}...")
    ret = os.system(f'insmod {BT_MODULE} bt_mac={bt_mac}')
    if ret != 0:
        print(f"insmod failed (exit {ret})")
        return False

    # Wait for hci0
    print("Waiting for hci0...")
    for i in range(20):
        if os.path.exists(HCI_SYSFS):
            print(f"hci0 appeared after {(i+1)*0.5:.1f}s")
            return True
        time.sleep(0.5)

    print("WARNING: hci0 did not appear after 10s")
    return False


def format_settings(bits):
    """Decode settings bitmask."""
    names = [
        "powered", "connectable", "fast-connectable", "discoverable",
        "bondable", "link-security", "ssp", "br/edr",
        "hs", "le", "advertising", "secure-conn",
        "debug-keys", "privacy", "configuration", "static-addr",
    ]
    active = []
    for i, name in enumerate(names):
        if bits & (1 << i):
            active.append(name)
    return ', '.join(active) if active else 'none'


def parse_device_found(data):
    """Parse DEVICE_FOUND event data."""
    if len(data) < 14:
        return f"(too short: {data.hex(' ')})"
    addr = data[0:6]
    addr_type = data[6]
    rssi = struct.unpack("<b", data[7:8])[0]
    flags = struct.unpack("<I", data[8:12])[0]
    eir_len = struct.unpack("<H", data[12:14])[0]
    eir_data = data[14:14+eir_len]

    addr_str = ':'.join(f'{b:02X}' for b in reversed(addr))
    addr_type_str = "BR/EDR" if addr_type == 0 else f"LE({addr_type})"

    # Parse EIR for device name
    name = ""
    pos = 0
    while pos < len(eir_data) - 1:
        elen = eir_data[pos]
        if elen == 0:
            break
        if pos + elen >= len(eir_data):
            break
        etype = eir_data[pos + 1]
        evalue = eir_data[pos + 2:pos + 1 + elen]
        if etype in (0x08, 0x09):  # Shortened/Complete Local Name
            name = evalue.decode('utf-8', errors='replace')
        pos += elen + 1

    return f"{addr_str} ({addr_type_str}) RSSI={rssi}dBm name='{name}'"


def cmd_monitor(mgmt):
    """Monitor all management events."""
    print("Monitoring Bluetooth management events. Ctrl+C to stop.")
    print("-" * 70)
    mgmt.sock.settimeout(None)  # Block forever
    try:
        while True:
            try:
                raw = mgmt.sock.recv(4096)
            except KeyboardInterrupt:
                break
            if len(raw) < 6:
                continue
            opcode, index, plen = struct.unpack("<HHH", raw[:6])
            data = raw[6:6+plen]
            name = MGMT_EVENT_NAMES.get(opcode, f"UNKNOWN(0x{opcode:04x})")
            ts = time.strftime('%H:%M:%S')

            detail = ""
            if opcode == MGMT_EV_DEVICE_FOUND:
                detail = " → " + parse_device_found(data)
            elif opcode == MGMT_EV_CMD_COMPLETE and len(data) >= 3:
                cmd_op, status = struct.unpack("<HB", data[:3])
                detail = f" → cmd=0x{cmd_op:04x} status={status}"
            elif opcode == MGMT_EV_NEW_SETTINGS and len(data) >= 4:
                settings = struct.unpack("<I", data[:4])[0]
                detail = f" → {format_settings(settings)}"
            elif opcode == 0x000F and len(data) >= 7:  # USER_CONFIRM_REQUEST
                addr = data[0:6]
                addr_str = ':'.join(f'{b:02X}' for b in reversed(addr))
                detail = f" → {addr_str} (auto-confirming...)"
                # Auto-confirm pairing
                mgmt.confirm_reply(data[0:6], data[6])

            print(f"[{ts}] {name} idx={index} len={plen}{detail}")
            if data and opcode not in (MGMT_EV_DEVICE_FOUND, MGMT_EV_CMD_COMPLETE, MGMT_EV_NEW_SETTINGS):
                # Show raw hex for less common events
                print(f"         data: {data.hex(' ')}")

    except KeyboardInterrupt:
        pass
    print("\nStopped.")


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)

    cmd = sys.argv[1].lower()

    if cmd == "load":
        load_bt_module()
        return

    if cmd == "monitor":
        mgmt = MgmtSocket()
        try:
            cmd_monitor(mgmt)
        finally:
            mgmt.close()
        return

    # All other commands need the mgmt socket
    mgmt = MgmtSocket()
    try:
        if cmd == "info":
            ver = mgmt.read_version()
            if ver:
                print(f"Mgmt API version: {ver[0]}.{ver[1]}")

            info = mgmt.read_info()
            if info:
                print(f"BD Address:   {info.get('address', '?')}")
                print(f"BT Version:   {info.get('bt_version', '?')}")
                print(f"Manufacturer: {info.get('manufacturer', '?')}")
                print(f"Name:         {info.get('name', '?')}")
                print(f"Short Name:   {info.get('short_name', '?')}")
                sup = info.get('supported_settings', 0)
                cur = info.get('current_settings', 0)
                print(f"Supported:    {format_settings(sup)}")
                print(f"Current:      {format_settings(cur)}")
            else:
                print("Failed to read adapter info (is bt8xxx loaded?)")

        elif cmd == "up":
            print("Powering up Bluetooth adapter...")

            print("  Setting IO capability (NoInputNoOutput)...")
            mgmt.set_io_capability(0x03)

            print("  Loading empty link keys...")
            mgmt.load_link_keys()

            print("  Enabling SSP...")
            mgmt.set_ssp(True)

            print("  Setting powered ON...")
            result = mgmt.set_powered(True)
            if result:
                print(f"  → status={result[0]}")

            print("  Setting connectable...")
            mgmt.set_connectable(True)

            print("  Setting discoverable (180s)...")
            mgmt.set_discoverable(True, timeout_sec=180)

            print("  Setting pairable...")
            mgmt.set_pairable(True)

            print("  Setting name to 'HK Invoke'...")
            mgmt.set_local_name("HK Invoke", "Invoke")

            print("Bluetooth adapter is UP and discoverable.")
            print("Run 'bt_discover.py monitor' to watch for connections.")

        elif cmd == "down":
            print("Powering down...")
            result = mgmt.set_powered(False)
            if result:
                print(f"  → status={result[0]}")
            print("Bluetooth adapter is OFF.")

        elif cmd == "scan":
            print("Starting BR/EDR discovery (10s)...")
            result = mgmt.start_discovery(br_edr=True, le=False)
            if result and result[0] != 0:
                print(f"  start_discovery failed: status={result[0]}")
                return

            print("Scanning... Ctrl+C to stop early.")
            deadline = time.time() + 12
            try:
                while time.time() < deadline:
                    evt = mgmt.recv_event(timeout=1.0)
                    if evt is None:
                        continue
                    opcode, index, data = evt
                    name = MGMT_EVENT_NAMES.get(opcode, f"0x{opcode:04x}")
                    if opcode == MGMT_EV_DEVICE_FOUND:
                        print(f"  FOUND: {parse_device_found(data)}")
                    elif opcode == MGMT_EV_DISCOVERING and len(data) >= 2:
                        discovering = data[1]
                        if not discovering:
                            print("  Discovery complete.")
                            break
                    else:
                        print(f"  ({name})")
            except KeyboardInterrupt:
                print("\n  Stopping discovery...")
                mgmt.stop_discovery()

        elif cmd == "pair":
            print("Enabling pairing mode (Just Works)...")
            mgmt.set_io_capability(0x03)
            mgmt.set_pairable(True)
            print("Pairing enabled. Run 'bt_discover.py monitor' to auto-confirm.")
            print("Or run 'bt_discover.py up' first to make adapter discoverable.")

        elif cmd == "hci":
            if len(sys.argv) < 3:
                print("Usage: bt_discover.py hci AABBCCDD  (raw hex)")
                sys.exit(1)
            raw = bytes.fromhex(sys.argv[2])
            print(f"Sending raw: {raw.hex(' ')}")
            mgmt.sock.send(raw)
            time.sleep(0.5)
            evt = mgmt.recv_event(timeout=2.0)
            if evt:
                opcode, index, data = evt
                name = MGMT_EVENT_NAMES.get(opcode, f"0x{opcode:04x}")
                print(f"Response: {name} idx={index} data={data.hex(' ')}")

        else:
            print(f"Unknown command: {cmd}")
            print(__doc__)
            sys.exit(1)

    finally:
        mgmt.close()


if __name__ == "__main__":
    main()
