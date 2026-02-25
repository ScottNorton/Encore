#!/usr/bin/env python3
"""Capture serial data at a specific baud rate for analysis."""
import serial
import sys
import time

port = sys.argv[1] if len(sys.argv) > 1 else "COM8"
baud = int(sys.argv[2]) if len(sys.argv) > 2 else 9600
duration = int(sys.argv[3]) if len(sys.argv) > 3 else 10

print(f"Capturing {port} at {baud} baud for {duration}s...")
print("Press buttons, turn volume, interact with device!\n")

ser = serial.Serial(port, baud, timeout=0.5)
ser.reset_input_buffer()

all_data = b''
start = time.time()
while time.time() - start < duration:
    chunk = ser.read(512)
    if chunk:
        all_data += chunk
        ts = f"{time.time()-start:.1f}s"
        print(f"[{ts}] +{len(chunk)} bytes (total: {len(all_data)})")

ser.close()

if all_data:
    printable = sum(1 for b in all_data if 32 <= b <= 126 or b in (10, 13))
    pct = printable / len(all_data) * 100
    print(f"\nTotal: {len(all_data)} bytes, {pct:.0f}% printable")
    print(f"\nFull hex dump:")
    for i in range(0, len(all_data), 16):
        row = all_data[i:i+16]
        hex_str = ' '.join(f'{b:02x}' for b in row)
        ascii_str = ''.join(chr(b) if 32 <= b <= 126 else '.' for b in row)
        print(f"  {i:04x}: {hex_str:<48} {ascii_str}")
    print(f"\nASCII attempt:")
    print(f"  {all_data.decode('ascii', errors='replace')[:200]}")
else:
    print("No data received.")
