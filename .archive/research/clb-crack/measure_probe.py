#!/usr/bin/env python3
"""Probe: full value ranges per DID/form, CSV time cadence, capture overlap."""
import sys, os
from collections import defaultdict, Counter
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_coolant as M

series, _ = M.extract()
csvs = M.parse_csv()

# capture time span
allt = [t for did in series for t, _ in series[did]]
print(f"# capture t: [{min(allt):.1f}, {max(allt):.1f}] span={max(allt)-min(allt):.1f}s, n={len(allt)}")
for m in csvs:
    ts = [t for t, _ in m["series"]]
    print(f"# CSV {m['id']} t:[{min(ts):.1f},{max(ts):.1f}] dt~{(max(ts)-min(ts))/(len(ts)-1):.2f} n={len(ts)}")

print("\n# per-DID full value-range by form:")
for did in sorted(series):
    s = sorted(series[did])
    print(f"\nDID {did:04X}  (n={len(s)}, len0={len(s[0][1])})")
    for fn, fv in M.FORMS.items():
        try:
            rv = [fv(v) for _, v in s]
        except Exception:
            continue
        if len(set(rv)) < 2:
            print(f"    {fn:6s} CONST {rv[0]}")
            continue
        print(f"    {fn:6s} range=[{min(rv)},{max(rv)}] distinct={len(set(rv))}")

# show 7458 and A03B full detail with times
for did in [0x7458, 0x7450, 0xA03B, 0xA0EF, 0x7444, 0x7410, 0x7419]:
    s = sorted(series[did])
    t0 = s[0][0]
    print(f"\n## DID {did:04X} series (t_rel, hex):")
    print("   " + " ".join(f"{t-t0:.0f}:{v.hex()}" for t, v in s[:40]))
