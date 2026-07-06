#!/usr/bin/env python3
"""extract_replay_stream.py -- flatten a captured HEX-cable diagnostic session
into an ordered replay stream for `vagcan replay-drive`.

The cable is a session-oriented transport: the engine ECU's diagnostic channel
(the "f3" channel, whose 16-byte block is enciphered with KS_F3) only comes up
after the WHOLE ordered setup sequence has been replayed from a fresh power-on.
This tool reads `research/reading-ecus.pcapng` with `usbpcap.reassemble_frames`
and emits every frame -- both directions, in wire order -- as JSONL:

    {"idx": N, "dir": "out"|"in", "payload": "<hex>"}

`payload` is the frame opcode byte + its data (i.e. the bytes after the S/M
marker+len, before the trailing XOR) -- exactly what `vagcan replay-drive`
re-sends (OUT) or expects back (IN).

It also locates the frame index at which the f3 engine channel becomes active,
VERIFIED from the data: the first frame whose block[0]==0xF3 decodes under KS_F3
to a sane single-frame UDS PDU (TesterPresent `3E` or ReadDataByIdentifier
`22 xx xx`). Output goes to `research/dumps/replay-stream.jsonl` (gitignored --
the stream carries the owner's VIN).

Pure research tooling; does not touch crates/.
"""
import json
import os
import sys

from usbpcap import reassemble_frames

# The f3 (engine ECU) channel keystream, mirrored from crates/vag-hex/src/link.rs
# (KS_F3). plain[i] = cipher[i] ^ KS_F3[i]. Only off6..13 (the UDS-bearing
# region) are recovered; the rest are 0.
KS_F3 = [0x00, 0xBD, 0x00, 0x00, 0x00, 0x00, 0x02, 0xA9,
         0x99, 0xF6, 0xDA, 0x7C, 0x9C, 0x3A, 0x00, 0x00]

# Block offsets (see link.rs / vag-hex-framing.md).
OFF_PCI = 6
OFF_SID = 7

DEFAULT_PCAP = os.path.join(os.path.dirname(__file__), "..", "reading-ecus.pcapng")
DEFAULT_OUT = os.path.join(os.path.dirname(__file__), "..", "dumps", "replay-stream.jsonl")


def decode_f3_uds(block):
    """XOR-decode a 16-byte block with KS_F3; return the single-frame UDS PDU
    (SID + data) if it is a sane ISO-TP single frame, else None."""
    if len(block) < 16:
        return None
    plain = [block[i] ^ KS_F3[i] for i in range(16)]
    pci = plain[OFF_PCI]
    if pci & 0xF0 != 0:  # not an ISO-TP single frame
        return None
    pdu_len = pci & 0x0F
    end = OFF_SID + pdu_len
    if pdu_len == 0 or end > 16:
        return None
    return plain[OFF_SID:end]


def is_sane_f3_uds(pdu):
    """A decoded PDU that looks like real engine-channel UDS: TesterPresent
    (3E ..) or ReadDataByIdentifier (22 xx xx)."""
    if not pdu:
        return False
    sid = pdu[0]
    if sid == 0x3E:  # TesterPresent
        return True
    if sid == 0x22 and len(pdu) >= 3:  # ReadDataByIdentifier
        return True
    return False


def main():
    pcap = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_PCAP
    out_path = sys.argv[2] if len(sys.argv) > 2 else DEFAULT_OUT

    frames = []  # ordered list of {"idx", "dir", "payload"(bytes)}
    f3_index = None

    for f in reassemble_frames(pcap):
        idx = len(frames)
        payload = f["payload"]  # opcode byte + data (no marker/len/xor)
        direction = "out" if f["dir"] == "OUT" else "in"
        frames.append({"idx": idx, "dir": direction, "payload": payload})

        # f3-channel detection: a b8/b7 diagnostic frame whose block starts
        # with 0xF3 and decodes under KS_F3 to a sane single-frame UDS PDU.
        # payload[0] = opcode (0xb8/0xb7), payload[1:17] = the 16-byte block.
        if f3_index is None and len(payload) >= 17 and payload[0] in (0xB8, 0xB7):
            block = payload[1:17]
            if block[0] == 0xF3:
                pdu = decode_f3_uds(block)
                if is_sane_f3_uds(pdu):
                    f3_index = idx

    # Write JSONL (gitignored dir; carries the owner's VIN).
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    with open(out_path, "w") as fh:
        for fr in frames:
            fh.write(json.dumps({
                "idx": fr["idx"],
                "dir": fr["dir"],
                "payload": fr["payload"].hex(),
            }) + "\n")

    out_count = sum(1 for fr in frames if fr["dir"] == "out")
    print(f"wrote {out_path}")
    print(f"total frames : {len(frames)}")
    print(f"OUT frames   : {out_count}")
    print(f"IN frames    : {len(frames) - out_count}")
    if f3_index is not None:
        fr = frames[f3_index]
        block = fr["payload"][1:17]
        pdu = decode_f3_uds(block)
        pdu_hex = " ".join(f"{b:02x}" for b in pdu) if pdu else "?"
        print(f"f3-channel index : {f3_index} (dir={fr['dir']}, decoded UDS = {pdu_hex})")
    else:
        print("f3-channel index : NOT FOUND (no KS_F3-decodable single-frame UDS)")

    # Sanity eyeball: leading OUT payload opcodes (expect plaintext bring-up
    # 02/09/04/82/0d then the b0..b6 setup burst).
    lead = [fr["payload"][0] for fr in frames if fr["dir"] == "out"][:24]
    print("leading OUT opcodes : " + " ".join(f"{op:02x}" for op in lead))


if __name__ == "__main__":
    main()
