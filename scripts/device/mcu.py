#!/usr/bin/env python3
"""MCU I2C driver for Harman Kardon Invoke.

Talks to the MSP430FR5739 MCU at I2C address 0x36 on /dev/i2c-0.
Provides LED control, button event monitoring, and BT LED toggle.

Usage:
    python3 mcu.py monitor     # Watch for button/touch/volume events
    python3 mcu.py btled on    # Turn on Bluetooth LED
    python3 mcu.py btled off   # Turn off Bluetooth LED
    python3 mcu.py init        # Run MCU init sequence
    python3 mcu.py led R G B   # Set all LEDs to RGB color (0-255 each)
    python3 mcu.py volume N    # Set volume arc (0-100)
    python3 mcu.py version     # Query MCU firmware version
    python3 mcu.py status      # Query MCU status register
    python3 mcu.py raw HH...   # Send raw hex bytes (6 bytes, zero-padded)
"""

import fcntl
import os
import struct
import sys
import time

I2C_SLAVE = 0x0703
I2C_BUS = "/dev/i2c-0"
MCU_ADDR = 0x36

LED_COUNT = 13
LED_FRAME_SIZE = LED_COUNT * 3  # 39 bytes


class MCU:
    def __init__(self, bus=I2C_BUS, addr=MCU_ADDR):
        self.fd = os.open(bus, os.O_RDWR)
        fcntl.ioctl(self.fd, I2C_SLAVE, addr)
        self.addr = addr
        self.vol_toggle = False

    def close(self):
        os.close(self.fd)

    def write(self, data):
        """Write bytes to MCU."""
        os.write(self.fd, bytes(data))
        time.sleep(0.005)  # 5ms post-write delay (MCU needs this)

    def read(self, n=6):
        """Read n bytes from MCU."""
        return os.read(self.fd, n)

    # ── Commands ──

    def version_query(self):
        """Send version query (0x01), read 6-byte response."""
        self.write([0x01, 0x00, 0x00, 0x00, 0x00, 0x00])
        time.sleep(0.1)
        resp = self.read(6)
        return resp

    def init(self):
        """Full MCU init sequence (required before LED frames work)."""
        print("MCU init: version query...")
        resp = self.version_query()
        print(f"  Version response: {resp.hex(' ')}")

        print("MCU init: LED ack...")
        self.write([0x23, 0x00, 0x00, 0x00, 0x6C, 0xBA])
        time.sleep(0.05)

        print("MCU init: draining events...")
        count = 0
        for _ in range(10):
            data = self.read(6)
            if data == b'\x00' * 6:
                break
            count += 1
            print(f"  Drained event: {data.hex(' ')}")
        print(f"  Drained {count} events")

        print("MCU init: device color...")
        self.write([0x25, 0x00, 0x00, 0x00, 0x00, 0x00])
        time.sleep(0.05)

        print("MCU init: status query...")
        self.write([0x26, 0x00, 0x00, 0x00, 0x00, 0x00])
        time.sleep(0.05)
        resp = self.read(6)
        print(f"  Status: {resp.hex(' ')}")
        print("MCU init complete.")

    def bt_led(self, on):
        """Turn Bluetooth LED on (True) or off (False). MCU cmd 0x0B."""
        val = 0x01 if on else 0x00
        self.write([0x0B, val, 0x00, 0x00, 0x00, 0x00])

    def bt_led_query(self):
        """Query Bluetooth LED state. MCU cmd 0x0C."""
        self.write([0x0C, 0x00, 0x00, 0x00, 0x00, 0x00])
        time.sleep(0.05)
        return self.read(6)

    def set_volume(self, level):
        """Set volume arc display (0-100). MCU cmd 0x03."""
        level = max(0, min(100, level))
        if self.vol_toggle:
            seq = [0xAC, 0xBE]
        else:
            seq = [0xAB, 0xB6]
        self.vol_toggle = not self.vol_toggle
        self.write([0x03, level, seq[0], seq[1], 0x00, 0x00])

    def set_led_rgb(self, r, g, b):
        """Set all LEDs to an RGB color using stock protocol.
        Sends [0x0E, 0x01, frame] to enter external mode with one frame."""
        frame = bytes([r, g, b] * LED_COUNT)  # 39 bytes
        buf = bytes([0x0E, 0x01]) + frame
        self.write(buf)

    def set_led_frames(self, frames, first=True):
        """Send LED frames using stock protocol [0x0E, flag, frames...].
        frames: list of 39-byte frames. first=True enters external mode."""
        flag = 0x01 if first else 0x00
        buf = bytes([0x0E, flag])
        for frame in frames:
            buf += bytes(frame)
        self.write(buf)

    def exit_led_mode(self):
        """Exit external LED mode, return to internal (volume arc). Cmd 0x05 0x21."""
        self.write([0x05, 0x21, 0x00, 0x00, 0x00, 0x00])

    def status_query(self):
        """Query MCU status register (0x26). Cleared after read."""
        self.write([0x26, 0x00, 0x00, 0x00, 0x00, 0x00])
        time.sleep(0.05)
        return self.read(6)

    def read_event(self):
        """Read a 6-byte event from MCU. Returns None if no event pending."""
        data = self.read(6)
        if data == b'\x00' * 6:
            return None
        return data

    def decode_event(self, data):
        """Decode a 6-byte MCU event into a human-readable string."""
        if data is None:
            return None
        b0, b1, b2, b3, b4, b5 = data
        if b0 == 0x04:
            events = {
                0x00: "Touch SHORT press",
                0x01: "Touch LONG press",
                0x02: "Bluetooth SHORT press",
                0x03: "Bluetooth LONG press",
                0x04: "Mic mute SHORT press",
                0x05: "Mic mute LONG press",
                0x06: "Reset SHORT press",
                0x07: "Reset LONG press",
                0x08: f"Volume UP (steps={b2})",
                0x09: f"Volume DOWN (steps={b2})",
                0x0A: "BT+MIC combo long press",
            }
            name = events.get(b1, f"Unknown button 0x{b1:02x}")
            return f"BUTTON: {name}"
        elif b0 == 0x01 and b1 == 0x01:
            return f"VERSION: {b3}.{b4}.{b5}"
        else:
            return f"UNKNOWN: {data.hex(' ')}"

    def monitor(self, poll_hz=30):
        """Poll MCU for events and print them. Ctrl+C to stop."""
        interval = 1.0 / poll_hz
        print(f"Monitoring MCU events at {poll_hz}Hz. Press Ctrl+C to stop.")
        print("Try: touch the top, press BT/mic/reset buttons, turn volume ring")
        print("-" * 60)
        try:
            while True:
                data = self.read_event()
                if data is not None:
                    desc = self.decode_event(data)
                    raw = data.hex(' ')
                    print(f"[{time.strftime('%H:%M:%S')}] {desc}  (raw: {raw})")
                time.sleep(interval)
        except KeyboardInterrupt:
            print("\nStopped.")


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)

    cmd = sys.argv[1].lower()
    mcu = MCU()

    try:
        if cmd == "monitor":
            hz = int(sys.argv[2]) if len(sys.argv) > 2 else 30
            mcu.monitor(poll_hz=hz)

        elif cmd == "btled":
            if len(sys.argv) < 3:
                # Query current state
                resp = mcu.bt_led_query()
                print(f"BT LED state: {resp.hex(' ')} (byte1={'ON' if resp[1] else 'OFF'})")
            elif sys.argv[2].lower() in ("on", "1", "true"):
                mcu.bt_led(True)
                print("BT LED: ON")
            else:
                mcu.bt_led(False)
                print("BT LED: OFF")

        elif cmd == "init":
            mcu.init()

        elif cmd == "led":
            if len(sys.argv) < 5:
                print("Usage: mcu.py led R G B  (0-255 each)")
                sys.exit(1)
            r, g, b = int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
            mcu.set_led_rgb(r, g, b)
            print(f"LEDs set to RGB({r}, {g}, {b})")

        elif cmd == "volume":
            if len(sys.argv) < 3:
                print("Usage: mcu.py volume N  (0-100)")
                sys.exit(1)
            level = int(sys.argv[2])
            mcu.set_volume(level)
            print(f"Volume arc: {level}")

        elif cmd == "version":
            resp = mcu.version_query()
            print(f"Version response: {resp.hex(' ')}")
            if resp[0] == 0x01 and resp[1] == 0x01:
                print(f"  Firmware: {resp[3]}.{resp[4]}.{resp[5]}")

        elif cmd == "status":
            resp = mcu.status_query()
            print(f"Status: {resp.hex(' ')}")

        elif cmd == "raw":
            if len(sys.argv) < 3:
                print("Usage: mcu.py raw AABBCCDDEE  (hex, zero-padded to 6 bytes)")
                sys.exit(1)
            raw = bytes.fromhex(sys.argv[2])
            # Pad to 6 bytes
            raw = raw + b'\x00' * (6 - len(raw))
            print(f"Sending: {raw.hex(' ')}")
            mcu.write(list(raw))
            time.sleep(0.05)
            resp = mcu.read(6)
            print(f"Response: {resp.hex(' ')}")

        else:
            print(f"Unknown command: {cmd}")
            print(__doc__)
            sys.exit(1)

    finally:
        mcu.close()


if __name__ == "__main__":
    main()
