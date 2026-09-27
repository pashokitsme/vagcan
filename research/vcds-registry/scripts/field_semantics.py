"""For ODIS rows whose (id, DID) exists in RM, test each RM field reading against ODIS:
position (f7/f8 vs bit_offset), length, signedness (f2&128), byte order (f2&64), scaling type and factor/offset.
Each ODIS row is compared with the RM rows of the same id+DID+len; reported as 'any RM row agrees'."""
import sys, os, collections
sys.path.insert(0, os.path.dirname(__file__))
from common import *
tt = load_tttext(); rm = load_rm(); od = load_odis()
by = collections.defaultdict(list)
for k, i, r in rm:
    if r is None or r["did"] is None: continue
    x = odx_id(tt.get(k, "")) if tt.get(k) else None
    if x: by[(x, r["did"])].append(r)
def fac(r):
    if r["num"] is None and r["den"] is None: return None
    return r["num"] / r["den"] if r["den"] else None
def close(a, b): return a is not None and b is not None and abs(a - b) <= 1e-6 * max(1.0, abs(a), abs(b))
st = collections.Counter(); pos = collections.Counter(); f2c = collections.Counter()
for o in od:
    x = o["text_id"]
    if not x or x[:3] not in ("IDE", "MAS"): continue
    st[x[:3] + " total"] += 1
    rows = by.get((x, o["did"]))
    if not rows: continue
    st[x[:3] + " id+DID in RM"] += 1
    L = [r for r in rows if r["len"] == o["bit_length"]]
    if not L: st["len mismatch"] += 1; continue
    st["id+DID+len"] += 1
    # position candidates
    for name, fn in (("byte*8+bit", lambda r: r["byte"] * 8 + r["bit"]),
                     ("byte*8+(8-len%8)-bit", lambda r: r["byte"] * 8 + r["bit"]),):
        pass
    P = [r for r in L if r["byte"] is not None and r["bit"] is not None]
    if any(r["byte"] * 8 + r["bit"] == o["bit_offset"] for r in P): pos["byte*8+bit == ODIS bit_offset"] += 1
    if any(r["byte"] * 8 + (7 - r["bit"]) == o["bit_offset"] for r in P): pos["byte*8+(7-bit)"] += 1
    if any(r["byte"] * 8 == o["bit_offset"] - (o["bit_offset"] % 8) for r in P): pos["byte only"] += 1
    if o["bit_offset"] % 8 == 0 and o["bit_length"] % 8 == 0: pos["byte-aligned rows"] += 1
    # flags
    sg = [r for r in L if r["f2"].isdigit() and ((int(r["f2"]) & 128) != 0) == bool(o["signed"])]
    if sg: st["signed agrees (f2&128)"] += 1
    if o["bit_length"] > 8:
        st["multi-byte rows"] += 1
        be = [r for r in L if r["f2"].isdigit() and ((int(r["f2"]) & 64) == 0) == bool(o["big_endian"])]
        if be: st["  byte order agrees (f2&64 = LE)"] += 1
    if o["scaling"] == "linear":
        st["linear"] += 1
        lin = [r for r in L if r["f2"].isdigit() and (int(r["f2"]) & 63) in (0,)]
        if any(close(fac(r), o["factor"]) and close(r["off"] if r["off"] is not None else 0.0, o["offset"] or 0.0) for r in lin): st["  linear: f2 base 0 with factor & offset agreeing"] += 1
        ident = [r for r in L if r["f2"].isdigit() and (int(r["f2"]) & 63) == 2]
        if ident and close(o["factor"], 1.0) and close(o["offset"] or 0.0, 0.0): st["  linear x1: f2 base 2 present"] += 1
        if close(o["factor"], 1.0) and close(o["offset"] or 0.0, 0.0): st["  linear x1 total"] += 1
    elif o["scaling"] == "enum":
        st["enum"] += 1
        if any(r["f2"].isdigit() and (int(r["f2"]) & 63) == 3 for r in L): st["  enum: f2 base 3 present"] += 1
    for r in L: f2c[(o["scaling"], (int(r["f2"]) & 63) if r["f2"].isdigit() else r["f2"])] += 1
for k, v in sorted(st.items()): print(f"  {k:52s} {v}")
for k, v in sorted(pos.items()): print(f"  pos {k:48s} {v}")
print("  (ODIS scaling, RM f2 base) among same id+DID+len rows:", f2c.most_common(20))
