#!/usr/bin/env python3
"""off14_rule.py -- RE the host b8 counter (off14) rule from reading-ecus.pcapng.

For every b8 (OUT) / b7 (IN) diagnostic frame, the inner 16-byte block starts at
payload[1] (payload[0] is the opcode). Block offsets:
  off0 = payload[1], off1 = SID echo, off2..5 = payload[3..6], off4 = direction,
  off14 = payload[15], off15 = payload[16].

We key a *channel* by (off0, off2, off3, off5) -- the direction-independent header
bytes -- and print the interleaved OUT/IN off14 stream in wire order to reverse
how VCDS picks off14 for each OUT b8 relative to the cable's b7 off14.
"""
import sys
from usbpcap import reassemble_frames

PATH = sys.argv[1] if len(sys.argv) > 1 else "../reading-ecus.pcapng"


def chan_key(blk):
    # direction-independent channel id: off0, off2, off3, off5
    return (blk[0], blk[2], blk[3], blk[5])


def main():
    events = []  # (first_idx, dir, chan, off4, off14, off15, blk)
    for f in reassemble_frames(PATH):
        op = f["payload"][:1].hex()
        if op not in ("b8", "b7"):
            continue
        pl = f["payload"]
        if len(pl) < 17:
            continue
        blk = pl[1:17]
        events.append((f["first_idx"], f["dir"], chan_key(blk),
                       blk[4], blk[14], blk[15], blk))

    print(f"total b8/b7 diag frames: {len(events)}")

    # Per-channel interleaved stream (both directions).
    from collections import defaultdict
    by_chan = defaultdict(list)
    for e in events:
        by_chan[e[2]].append(e)

    print(f"\n# channels: {len(by_chan)}")
    for chan, evs in sorted(by_chan.items(), key=lambda kv: -len(kv[1])):
        n_out = sum(1 for e in evs if e[1] == "OUT")
        n_in = sum(1 for e in evs if e[1] == "IN")
        print(f"\n=== chan off0/2/3/5 = {chan[0]:02x} {chan[1]:02x} {chan[2]:02x} "
              f"{chan[3]:02x}  ({len(evs)} frames: {n_out} OUT / {n_in} IN)")
        # print the first ~60 events interleaved
        for (idx, d, _c, off4, off14, off15, blk) in evs[:60]:
            print(f"  {idx:7d} {d:3s} off4={off4:02x} off14={off14:02x} "
                  f"off15={off15:02x}")


if __name__ == "__main__":
    main()
