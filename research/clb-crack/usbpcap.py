#!/usr/bin/env python3
"""usbpcap.py -- extract FTDI bulk transfer payloads from a USBPcap (.pcapng)
capture, for reversing the cable wire format (vag-hex interop).

USBPcap pseudo-header (LINKTYPE_USBPCAP=249), little-endian:
  off size field
  0   2    headerLen (0x1c = 28)
  2   8    irpId
  10  4    status
  14  2    function
  16  1    info (bit0 = 0:host->dev / 1:dev->host, i.e. direction of PDO)
  17  2    bus
  19  2    device
  21  1    endpoint (bit7 = IN)
  22  1    transfer (0=iso,1=intr,2=ctrl,3=bulk)
  23  4    dataLength
  27  ...  (control extra 8 bytes if ctrl) then payload

FTDI D2XX bulk: every IN transfer is prefixed with a 2-byte modem/line status
(0x01 0x60 typically) that is NOT part of the wire payload -- stripped here.
Pure research tooling; does not touch crates/.
"""
import sys
import struct
from scapy.all import PcapNgReader

TT = {0: "iso", 1: "intr", 2: "ctrl", 3: "bulk"}


def parse_records(path):
    """Yield dicts for every USB transfer record in the capture."""
    rdr = PcapNgReader(path)
    for idx, pkt in enumerate(pkt_iter(rdr)):
        b = bytes(pkt)
        if len(b) < 28:
            continue
        hlen = struct.unpack_from("<H", b, 0)[0]
        if hlen < 27:
            continue
        (irp,) = struct.unpack_from("<Q", b, 2)
        status = struct.unpack_from("<I", b, 10)[0]
        func = struct.unpack_from("<H", b, 14)[0]
        info = b[16]
        bus = struct.unpack_from("<H", b, 17)[0]
        dev = struct.unpack_from("<H", b, 19)[0]
        ep = b[21]
        tt = b[22]
        dlen = struct.unpack_from("<I", b, 23)[0]
        payload = b[hlen:]
        yield {
            "idx": idx,
            "irp": irp,
            "status": status,
            "func": func,
            "info": info,
            "bus": bus,
            "dev": dev,
            "ep": ep,
            "dir_in": bool(ep & 0x80),
            "tt": TT.get(tt, str(tt)),
            "dlen": dlen,
            "data": payload,
        }


def pkt_iter(rdr):
    for pkt in rdr:
        yield pkt


def ftdi_bulk_stream(path, strip_status=True):
    """Yield (idx, dir, bytes) for bulk transfers carrying real payload.

    dir is 'OUT' (host->cable) or 'IN' (cable->host). FTDI IN transfers carry a
    2-byte status prefix (modem+line status) on EACH 64-byte USB packet; with
    strip_status we drop the leading 2 bytes of each IN transfer (best-effort;
    multi-packet IN needs per-64B destatus -- flagged, see strip note).
    """
    for r in parse_records(path):
        if r["tt"] != "bulk":
            continue
        data = r["data"]
        if not data:
            continue
        if r["dir_in"]:
            payload = strip_ftdi_in(data) if strip_status else data
            if payload:
                yield (r["idx"], "IN", payload)
        else:
            yield (r["idx"], "OUT", data)


def strip_ftdi_in(data):
    """Strip the 2-byte FTDI status that prefixes every 64-byte USB IN packet.

    FTDI hardware inserts [modem_status, line_status] at the head of each
    packet of <=64 bytes. For a bulk transfer reassembled across N 64-byte
    packets, the status pair repeats every 64 bytes. We destatus per 64B block.
    """
    out = bytearray()
    for i in range(0, len(data), 64):
        block = data[i:i + 64]
        if len(block) >= 2:
            out += block[2:]
    return bytes(out)


def reassemble_frames(path):
    """Reassemble the byte-stream framing over FTDI bulk into whole frames.

    Confirmed wire format (from capture, supersedes the static 3-layer model):
      host->cable:  0x53 'S'  len  payload...  xor
      cable->host:  0x4D 'M'  len  payload...  xor
    `len` = total frame length incl marker+len+xor. `xor` = XOR over all
    preceding bytes (marker..last payload byte), init 0. FTDI bulk carries this
    as a raw byte stream; a frame spans multiple USB transfers, so we buffer per
    direction and cut on `len`.

    Yields dicts: {dir, marker, length, payload, xor_ok, first_idx, raw}.
    """
    bufs = {"OUT": bytearray(), "IN": bytearray()}
    idxs = {"OUT": None, "IN": None}
    for idx, d, data in ftdi_bulk_stream(path):
        buf = bufs[d]
        if idxs[d] is None:
            idxs[d] = idx
        buf += data
        while True:
            frame = _cut_frame(buf, d)
            if frame is None:
                break
            marker, length, payload, xor_byte, raw = frame
            yield {
                "dir": d,
                "marker": marker,
                "length": length,
                "payload": bytes(payload),
                "xor_ok": xor_cksum(raw[:-1]) == xor_byte,
                "first_idx": idxs[d],
                "raw": bytes(raw),
            }
            idxs[d] = idx  # next frame's approx start


def _cut_frame(buf, d):
    """If `buf` holds a complete frame at its head, pop and return it."""
    want_marker = 0x53 if d == "OUT" else 0x4D
    # resync: drop bytes until a plausible marker leads the buffer
    while buf and buf[0] != want_marker:
        del buf[0]
    if len(buf) < 3:
        return None
    length = buf[1]
    if length < 3:  # nonsense length; drop marker, resync
        del buf[0]
        return None
    if len(buf) < length:
        return None
    raw = bytes(buf[:length])
    marker = raw[0]
    payload = raw[2:length - 1]
    xor_byte = raw[length - 1]
    del buf[:length]
    return marker, length, payload, xor_byte, raw


def xor_cksum(bs):
    x = 0
    for b in bs:
        x ^= b
    return x


def summary(path):
    tt_ct = {}
    ep_ct = {}
    n = 0
    for r in parse_records(path):
        n += 1
        tt_ct[r["tt"]] = tt_ct.get(r["tt"], 0) + 1
        key = ("IN" if r["dir_in"] else "OUT", hex(r["ep"]))
        ep_ct[key] = ep_ct.get(key, 0) + 1
    print(f"{path}: {n} records")
    print("  transfer types:", tt_ct)
    print("  endpoints:", ep_ct)


if __name__ == "__main__":
    path = sys.argv[1] if len(sys.argv) > 1 else "../init-only.pcapng"
    mode = sys.argv[2] if len(sys.argv) > 2 else "summary"
    if mode == "summary":
        summary(path)
    elif mode == "bulk":
        for idx, d, payload in ftdi_bulk_stream(path):
            print(f"{idx:6d} {d:3s} {len(payload):4d}  {payload.hex()}")
    elif mode == "frames":
        bad = 0
        for f in reassemble_frames(path):
            flag = "" if f["xor_ok"] else "  !!XOR"
            if not f["xor_ok"]:
                bad += 1
            print(f"{f['first_idx']:6d} {f['dir']:3s} len={f['length']:3d} "
                  f"cmd={f['payload'][:1].hex() or '--':2s} "
                  f"payload={f['payload'].hex()}{flag}")
        if bad:
            print(f"# {bad} frames failed XOR", file=sys.stderr)
