#!/usr/bin/env python3
"""usbflood — floods the dash board's USB cable with 4095-byte framed requests.

What it is for: `todo/dash/17-bench-ble-usb.md` §2 item 13 — a USB host sending the largest
requests the link can frame, as fast as the board takes them, while BLE runs beside it. On
2026-09-22 it found three faults (`research/dash/can-bring-up.md` §9.14): a heap panic, a USB
read that never woke again, and a panic while a frame was gathered. Bench only, never a car:
every request goes to 7E0 (`22` and 2047 × `F4 0D`); the board refuses them without touching
the bus since then, but an older image would forward them.

Input:  the board's serial port and a duration in seconds, e.g.
        python3 research/dash/usbflood.py /dev/cu.usbmodem101 60 flood.log
        The board runs the `dash` image; `benchecu` answering 7E0 is optional.
Output: flood.log — every byte the board sent back meanwhile (FRAME lines, notes, link
        answers); count `PANIC`, `request over 64 bytes` and `this end's` in it. On stdout
        one line: requests started, bytes written and the rate — the rate falls when the
        board holds the host back, which is the expected answer to a flood.

It says Hello first, never blocks on a write (non-blocking I/O, stops on time), reads for 5 s
after the flood, then closes the port. Killing it mid-write instead once left macOS draining
a half-sent frame into a board that no longer read.
"""
import os, sys, time, tty, struct, select, errno
port, secs, out = sys.argv[1], float(sys.argv[2]), sys.argv[3]
fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK); tty.setraw(fd)
def msg(t, body): return b"\x00" + bytes([t]) + struct.pack("<H", len(body)) + body
pdu = b"\x22" + b"\xf4\x0d" * 2047
log = open(out, "wb"); pend = msg(0x07, b""); seq = 0; sent = 0; wrote = 0
t0 = time.time(); stop = t0 + secs
while time.time() < stop:
    r, w, _ = select.select([fd], [fd] if pend else [], [], 0.05)
    if r:
        try: log.write(os.read(fd, 65536))
        except OSError as e:
            if e.errno != errno.EAGAIN: raise
    if not pend:
        pend = msg(0x01, bytes([seq]) + struct.pack("<HH", 0x7E0, 0x7E8) + pdu); seq = (seq + 1) & 0xFF; sent += 1
    if w:
        try: n = os.write(fd, pend); wrote += n; pend = pend[n:]
        except OSError as e:
            if e.errno != errno.EAGAIN: raise

end = time.time() + 5
while time.time() < end:
    r, _, _ = select.select([fd], [], [], 0.2)
    if r:
        try: log.write(os.read(fd, 65536))
        except OSError: pass
unsent = len(pend)
a = time.time(); os.close(fd); c = time.time() - a
print(f"requests started {sent}, bytes written {wrote} ({wrote/secs/1024:.1f} KiB/s), {unsent} bytes of the last one unsent; close took {c:.2f} s")
