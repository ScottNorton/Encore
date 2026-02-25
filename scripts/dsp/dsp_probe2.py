#!/usr/bin/env python3
"""
DSP Probe v2 — Focused extraction from running ADSP-21489.

Key finding from v1: DSP_HAS_DATA (GPIO 12) is HIGH but DSP_READY (GPIO 15)
is LOW. This means we can READ events but can't SEND with GPIO handshake.
Simple SPI write+read works fine for commands.

Strategy:
  1. Read pending events via GPIO 12 toggle (no send handshake needed)
  2. Send commands via simple SPI write (works, confirmed in v1)
  3. Read responses via timed SPI reads
  4. Capture full 2048-byte buffers, not just first 64 bytes
"""
import os, sys, struct, fcntl, ctypes, time

SPI_IOC_WR_MODE  = 0x40016b01
SPI_IOC_WR_BITS  = 0x40016b03
SPI_IOC_WR_SPEED = 0x40046b04
SPI_IOC_MESSAGE_1 = 0x40206b00

GPIO_CS           = 4
GPIO_DSP_HAS_DATA = 12
GPIO_ARM_READY    = 13
GPIO_DSP_READY    = 15

def log(msg): print('[PROBE] %s' % msg, flush=True)

def gpio_export(pin):
    if not os.path.exists('/sys/class/gpio/gpio%d' % pin):
        with open('/sys/class/gpio/export', 'w') as f: f.write(str(pin))
        time.sleep(0.05)
def gpio_dir(pin, out):
    with open('/sys/class/gpio/gpio%d/direction' % pin, 'w') as f: f.write('out' if out else 'in')
def gpio_read(pin):
    with open('/sys/class/gpio/gpio%d/value' % pin, 'r') as f: return f.read().strip() == '1'
def gpio_write(pin, v):
    with open('/sys/class/gpio/gpio%d/value' % pin, 'w') as f: f.write('1' if v else '0')
def gpio_unexport(pin):
    try:
        with open('/sys/class/gpio/unexport', 'w') as f: f.write(str(pin))
    except: pass

class SPI:
    def __init__(self):
        self.fd = os.open('/dev/spidev0.0', os.O_RDWR)
        fcntl.ioctl(self.fd, SPI_IOC_WR_MODE, struct.pack('B', 3))
        fcntl.ioctl(self.fd, SPI_IOC_WR_BITS, struct.pack('B', 8))
        fcntl.ioctl(self.fd, SPI_IOC_WR_SPEED, struct.pack('I', 58824))

    def transfer(self, tx):
        tx_buf = ctypes.create_string_buffer(bytes(bytearray(tx)))
        rx_buf = ctypes.create_string_buffer(len(tx))
        xfer = struct.pack('QQIIHBBBBH',
            ctypes.addressof(tx_buf), ctypes.addressof(rx_buf),
            len(tx), 58824, 0, 8, 0, 0, 0, 0)
        fcntl.ioctl(self.fd, SPI_IOC_MESSAGE_1, xfer)
        return list(bytearray(rx_buf.raw))

    def write(self, data):
        os.write(self.fd, bytes(bytearray(data)))

    def read(self, n):
        return list(os.read(self.fd, n))

    def close(self):
        os.close(self.fd)


def build_msg(msg_type, data=None):
    data = data or []
    t_hi, t_lo = (msg_type >> 8) & 0xFF, msg_type & 0xFF
    l_hi, l_lo = (len(data) >> 8) & 0xFF, len(data) & 0xFF
    cksum = (t_hi + t_lo + l_hi + l_lo + sum(data)) & 0xFF
    msg = [t_hi, t_lo, l_hi, l_lo, cksum] + list(data)
    while len(msg) % 8: msg.append(0)
    return msg


def hex_dump(data, prefix='', max_bytes=256):
    if not data: return prefix + '(empty)'
    lines = []
    for i in range(0, min(len(data), max_bytes), 16):
        c = data[i:i+16]
        h = ' '.join('%02X' % b for b in c)
        a = ''.join(chr(b) if 32 <= b < 127 else '.' for b in c)
        lines.append('%s%04X: %-48s  %s' % (prefix, i, h, a))
    if len(data) > max_bytes:
        lines.append('%s... (%d more bytes)' % (prefix, len(data) - max_bytes))
    return '\n'.join(lines)


def find_nonzero_regions(data):
    """Find contiguous regions of non-zero bytes."""
    regions = []
    start = None
    for i, b in enumerate(data):
        if b != 0:
            if start is None: start = i
        else:
            if start is not None:
                regions.append((start, i - 1, data[start:i]))
                start = None
    if start is not None:
        regions.append((start, len(data) - 1, data[start:]))
    return regions


def main():
    log('ADSP-21489 Probe v2')
    spi = SPI()

    # ── Step 1: Set up GPIO ──
    log('\n=== GPIO Setup ===')
    for pin in [GPIO_CS, GPIO_DSP_HAS_DATA, GPIO_ARM_READY, GPIO_DSP_READY]:
        gpio_export(pin)
    gpio_dir(GPIO_CS, True)
    gpio_dir(GPIO_ARM_READY, True)
    gpio_dir(GPIO_DSP_HAS_DATA, False)
    gpio_dir(GPIO_DSP_READY, False)
    gpio_write(GPIO_CS, False)
    gpio_write(GPIO_ARM_READY, False)

    has_data = gpio_read(GPIO_DSP_HAS_DATA)
    dsp_ready = gpio_read(GPIO_DSP_READY)
    log('DSP_HAS_DATA=%d  DSP_READY=%d' % (has_data, dsp_ready))

    # ── Step 2: Drain pending events via GPIO read protocol ──
    log('\n=== Draining Pending Events ===')
    events = []
    for i in range(20):  # Max 20 events
        if not gpio_read(GPIO_DSP_HAS_DATA):
            log('DSP_HAS_DATA went LOW after %d events' % i)
            break

        # Toggle CS, assert ARM_READY, read, de-assert
        gpio_write(GPIO_CS, True)
        time.sleep(0.001)
        gpio_write(GPIO_CS, False)
        time.sleep(0.001)

        gpio_write(GPIO_ARM_READY, True)
        rx = spi.read(2048)
        gpio_write(GPIO_ARM_READY, False)

        # Parse event
        if len(rx) >= 3:
            cat = (rx[0] << 8) | rx[1]
            code = rx[2]
            if cat == 0 and code == 0 and (len(rx) < 4 or rx[3] == 0):
                log('  [%d] All-zero response (no event)' % i)
                break

            # Find meaningful data
            raw = rx[3:]
            while raw and raw[-1] == 0: raw.pop()

            EVENT_NAMES = {
                (0, 0x04): 'DacGain',
                (0, 0x05): 'ExpectSpeech',
                (0, 0x06): 'CancelTrigger',
                (0, 0x08): 'Version',
                (0, 0x09): 'MicMute',
                (0, 0x0C): 'MemoryDump',
                (1, 0x00): 'TRIGGER_FOUND',
                (1, 0x01): 'PayloadBegin',
                (1, 0x02): 'PayloadEnd',
                (1, 0x04): 'Bootup',
            }
            name = EVENT_NAMES.get((cat, code), 'Unknown')
            log('  [%d] cat=0x%04X code=0x%02X (%s) data_len=%d' % (i, cat, code, name, len(raw)))
            if raw:
                log(hex_dump(raw, '       ', max_bytes=64))
            events.append((cat, code, name, raw))
        time.sleep(0.01)

    log('Total events drained: %d' % len(events))

    # ── Step 3: Send commands and capture FULL responses ──
    log('\n=== Command Responses (full 2048-byte captures) ===')

    commands = [
        ('Version',    0x0000, [0x08]),
        ('MicMute?',   0x0000, [0x09]),
        ('MemDump p0', 0x0000, [0x0C, 0x00, 0x00]),
        ('MemDump p1', 0x0000, [0x0C, 0x00, 0x01]),
        ('HW Test',    0x0002, [0x0A]),
        ('Trigger',    0x0000, [0x0B]),  # trigger command
    ]

    for name, mtype, data in commands:
        log('\nCommand: %s (type=0x%04X data=%s)' % (
            name, mtype, ' '.join('%02X' % b for b in data)))

        # Send via simple write (no GPIO handshake needed)
        msg = build_msg(mtype, data)
        spi.transfer(msg)
        time.sleep(0.15)

        # Read full response
        rx = spi.read(2048)
        nz = sum(1 for b in rx if b != 0)
        log('  %d bytes, %d non-zero' % (len(rx), nz))

        if nz > 0:
            regions = find_nonzero_regions(rx)
            log('  Non-zero regions: %d' % len(regions))
            for start, end, rdata in regions:
                log('    [0x%04X-0x%04X] %d bytes:' % (start, end, len(rdata)))
                log(hex_dump(rdata, '      ', max_bytes=128))

            # Also check if it looks like an event frame
            if rx[0] != 0 or rx[1] != 0:
                cat = (rx[0] << 8) | rx[1]
                code = rx[2]
                log('  >> Looks like event: cat=0x%04X code=0x%02X' % (cat, code))

    # ── Step 4: Full memory dump of first 8 pages ──
    log('\n=== Memory Dump (Pages 0-7) ===')
    for page in range(8):
        msg = build_msg(0x0000, [0x0C, 0x00, page])
        spi.transfer(msg)
        time.sleep(0.2)
        rx = spi.read(2048)
        nz = sum(1 for b in rx if b != 0)
        log('Page %d: %d non-zero bytes' % (page, nz))
        if nz > 0:
            log(hex_dump(rx, '  ', max_bytes=256))

    # ── Step 5: Read a second time to check for stale data vs fresh ──
    log('\n=== Stale Data Check ===')
    log('Reading SPI without sending command (should be all zeros if no events)...')
    rx1 = spi.read(2048)
    nz1 = sum(1 for b in rx1 if b != 0)
    time.sleep(0.5)
    rx2 = spi.read(2048)
    nz2 = sum(1 for b in rx2 if b != 0)
    log('Read 1: %d non-zero' % nz1)
    log('Read 2: %d non-zero' % nz2)
    if rx1 == rx2 and nz1 > 0:
        log('SAME DATA — DSP holds last response in SPI buffer (stale)')
    elif nz1 == 0 and nz2 == 0:
        log('Both zero — DSP only sends data in response to commands')
    else:
        log('Different data — DSP is actively streaming')

    # ── Step 6: Check GPIO state after probing ──
    log('\n=== Final GPIO State ===')
    log('DSP_HAS_DATA=%d  DSP_READY=%d' % (
        gpio_read(GPIO_DSP_HAS_DATA), gpio_read(GPIO_DSP_READY)))

    # Cleanup
    for pin in [GPIO_CS, GPIO_DSP_HAS_DATA, GPIO_ARM_READY, GPIO_DSP_READY]:
        gpio_unexport(pin)

    spi.close()
    log('\nProbe complete.')


if __name__ == '__main__':
    main()
