#!/usr/bin/env python3
"""Listen on serial port continuously, trying both COM ports."""
import serial
import sys
import time

port = sys.argv[1] if len(sys.argv) > 1 else "COM7"
baud = int(sys.argv[2]) if len(sys.argv) > 2 else 9600

print(f"Listening on {port} at {baud} baud... Press buttons on device!")
print("Will listen for 15 seconds. Ctrl+C to stop early.\n")

ser = serial.Serial(port, baud, timeout=0.5)
start = time.time()
total = 0

while time.time() - start < 15:
    data = ser.read(256)
    if data:
        total += len(data)
        printable = sum(1 for b in data if 32 <= b <= 126 or b in (10, 13))
        pct = printable / len(data) * 100 if data else 0
        ts = f"{time.time()-start:.1f}s"
        print(f"[{ts}] {len(data)} bytes ({pct:.0f}% printable): hex={data[:20].hex(' ')}")
        try:
            print(f"       ascii: {data.decode('ascii', errors='replace')[:80]}")
        except:
            pass

ser.close()
print(f"\nDone. Total received: {total} bytes")
