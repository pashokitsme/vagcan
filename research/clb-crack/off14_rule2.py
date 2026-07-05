#!/usr/bin/env python3
"""off14_rule2.py -- temporal (idx-sorted) off14 analysis + req/resp relation."""
import sys
from collections import defaultdict
from usbpcap import reassemble_frames

PATH = sys.argv[1] if len(sys.argv) > 1 else "../reading-ecus.pcapng"


def chan_key(blk):
    return (blk[0], blk[2], blk[3], blk[5])


def main():
    events = []
    for f in reassemble_frames(PATH):
        op = f["payload"][:1].hex()
        if op not in ("b8", "b7"):
            continue
        pl = f["payload"]
        if len(pl) < 17:
            continue
        blk = pl[1:17]
        events.append({
            "idx": f["first_idx"], "dir": f["dir"], "chan": chan_key(blk),
            "off4": blk[4], "off14": blk[14], "off15": blk[15],
            "blk": blk.hex(),
        })

    events.sort(key=lambda e: e["idx"])

    # 1) Global temporal order for the 0x39 channel (auth), first 40.
    print("=== 0x39 channel (auth-completion), temporal order ===")
    n = 0
    for e in events:
        if e["chan"][0] != 0x39:
            continue
        print(f"  {e['idx']:7d} {e['dir']:3s} off4={e['off4']:02x} "
              f"off14={e['off14']:02x} off15={e['off15']:02x}  {e['blk']}")
        n += 1
        if n >= 40:
            break

    # 2) Req/resp off14 relation: for each channel, when an OUT is immediately
    #    followed (temporally, same channel) by an IN, record (out14, in14).
    print("\n=== req(OUT b8) -> resp(IN b7) off14 relation, per channel ===")
    per_chan = defaultdict(list)
    for e in events:
        per_chan[e["chan"]].append(e)
    rel_counts = defaultdict(int)
    for chan, evs in per_chan.items():
        for a, b in zip(evs, evs[1:]):
            if a["dir"] == "OUT" and b["dir"] == "IN":
                delta = (b["off14"] - a["off14"]) & 0xff
                xor = a["off14"] ^ b["off14"]
                rel_counts[(delta, xor)] += 1
    print("  (in14-out14 mod256, in14^out14) : count")
    for k, v in sorted(rel_counts.items(), key=lambda kv: -kv[1]):
        print(f"    delta={k[0]:02x} xor={k[1]:02x} : {v}")

    # 3) resp(IN) -> next req(OUT) relation (cable pushes, host answers next).
    print("\n=== resp(IN b7) -> next req(OUT b8) off14 relation, per channel ===")
    rel2 = defaultdict(int)
    for chan, evs in per_chan.items():
        for a, b in zip(evs, evs[1:]):
            if a["dir"] == "IN" and b["dir"] == "OUT":
                delta = (b["off14"] - a["off14"]) & 0xff
                xor = a["off14"] ^ b["off14"]
                rel2[(delta, xor)] += 1
    print("  (out14-in14 mod256, out14^in14) : count")
    for k, v in sorted(rel2.items(), key=lambda kv: -kv[1]):
        print(f"    delta={k[0]:02x} xor={k[1]:02x} : {v}")

    # 4) OUT->OUT consecutive delta per channel (host's own counter step).
    print("\n=== consecutive OUT b8 -> OUT b8 off14 delta (host self-step) ===")
    rel3 = defaultdict(int)
    for chan, evs in per_chan.items():
        outs = [e for e in evs if e["dir"] == "OUT"]
        for a, b in zip(outs, outs[1:]):
            delta = (b["off14"] - a["off14"]) & 0xff
            rel3[delta] += 1
    for k, v in sorted(rel3.items(), key=lambda kv: -kv[1])[:12]:
        print(f"    delta={k:02x} : {v}")


if __name__ == "__main__":
    main()
