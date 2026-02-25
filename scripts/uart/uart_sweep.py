#!/usr/bin/env python3
"""Sweep all baud rates on a COM port, listening 3 seconds each."""
import serial
import sys
import time

port = sys.argv[1] if len(sys.argv) > 1 else "COM7"
bauds = [9600, 115200, 57600, 38400, 19200, 4800, 2400, 1200, 230400, 460800, 1000000]

print(f"Sweeping {port} across {len(bauds)} baud rates (3s each)...")
print("Interact with the device during each test!\n")

for baud in bauds:
    try:
        ser = serial.Serial(port, baud, timeout=0.5)
        time.sleep(0.2)
        ser.reset_input_buffer()

        data = b''
        start = time.time()
        while time.time() - start < 3:
            chunk = ser.read(256)
            if chunk:
                data += chunk
        ser.close()

        if data:
            printable = sum(1 for b in data if 32 <= b <= 126 or b in (10, 13))
            pct = printable / len(data) * 100
            print(f"BAUD {baud:>7}: {len(data):>4} bytes, {pct:.0f}% printable, hex: {data[:30].hex(' ')}")
            try:
                print(f"           ascii: {data[:60].decode('ascii', errors='replace')}")
            except:
                pass
        else:
            print(f"BAUD {baud:>7}: silent")
    except Exception as e:
        print(f"BAUD {baud:>7}: ERROR - {e}")

print("\nDone.")
