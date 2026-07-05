#!/usr/bin/env python3
"""Extract the exact OUT-frame choreography from session start to the first
f3 diagnostic b8, so it can be replayed to reach the diagnostic state."""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from usbpcap import reassemble_frames


def main(path):
    frames = list(reassemble_frames(path))
    # find first f3 b8
    def is_b8(f):
        p = f["payload"]
        return f["dir"] == "OUT" and len(p) >= 17 and p[0] == 0xB8
    first = next(i for i, f in enumerate(frames) if is_b8(f))
    print(f"# first ANY b8 at frame list index {first} (idx {frames[first]['first_idx']})")
    counts = {}
    for f in frames[:first + 1]:
        p = f["payload"]
        op = p[0] if p else None
        counts.setdefault((f["dir"], op), 0)
        counts[(f["dir"], op)] += 1
    print("# opcode counts up to first b8:")
    for (d, op), n in sorted(counts.items(), key=lambda kv: (kv[0][0], kv[0][1] or 0)):
        label = f"{op:#04x}" if op is not None else "none"
        print(f"    {d} {label}: {n}")
    print("\n# OUT frames in order (opcode + payload), start..first b8:")
    n = 0
    for f in frames[:first + 1]:
        if f["dir"] != "OUT":
            continue
        p = f["payload"]
        n += 1
        print(f"  {n:3d} idx={f['first_idx']:6d} op={p[0]:02x} data={bytes(p[1:]).hex()}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "../reading-ecus.pcapng")
