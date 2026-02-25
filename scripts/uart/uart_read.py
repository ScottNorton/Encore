#!/usr/bin/env python3
"""Read from serial port at various baud rates to find the correct one."""
import serial
import sys
import time

port = sys.argv[1] if len(sys.argv) > 1 else "COM7"
bauds = [9600, 115200, 57600, 38400, 19200, 4800]

for baud in bauds:
    try:
        ser = serial.Serial(port, baud, timeout=2)
        time.sleep(0.5)
        data = ser.read(256)
        ser.close()
        if data:
            printable = sum(1 for b in data if 32 <= b <= 126 or b in (10, 13))
            pct = printable / len(data) * 100
            print(f"BAUD {baud}: {len(data)} bytes, {pct:.0f}% printable")
            print(f"  HEX: {data[:40].hex(' ')}")
            print(f"  ASCII: {data[:40]}")
            if pct > 70:
                print(f"  >>> LIKELY CORRECT BAUD RATE: {baud} <<<")
        else:
            print(f"BAUD {baud}: no data")
    except Exception as e:
        print(f"BAUD {baud}: error - {e}")
