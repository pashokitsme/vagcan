"""The owner's worked example: one ODX id across every opened unit — what the unit's own MWB selects in RM
versus what ODIS declares for that variant."""
import os, sys, json, collections
sys.path.insert(0, os.path.dirname(__file__))
from common import *
X = sys.argv[1] if len(sys.argv) > 1 else "IDE00075"
tt = load_tttext(); rm = load_rm(); od = load_odis()
srcs = json.load(open(os.path.join(S, "out/variant_sources.json")))
key = [k for k, t in tt.items() if odx_id(t) == X]
print(X, "-> TTTEXT record(s)", key, [tt[k] for k in key], "| RM rows under it:", sum(1 for k, i, r in rm if k in key))
def fmt(r):
    v = int(r["f2"]) if r["f2"].isdigit() else None
    base = v & 63 if v is not None else None
    sc = {0: f"x{r['num']}/{r['den']} {'+' + str(r['off']) if r['off'] else ''}", 2: "x1", 3: f"enum TTDOP {r['f1']}", 4: f"OBD PID {r['off']}", 7: "raw", 8: "ASCII"}.get(base, f"base{base}")
    return f"DID {r['did']:04X} bit {r['byte']*8+r['bit']:3d} len {r['len']:3d} {'s' if v & 128 else 'u'} {'LE' if v & 64 else 'BE'} {sc} unit#{r['unit']}"
for v in sorted(srcs):
    s = srcs[v]
    if not s["mwb"]: continue
    sel = []
    for p in s["mwb"]:
        for rid, code in read_unit_mwb(p):
            k, i, r = rm[rid - 1]
            if k in key and r is not None: sel.append((rid, code, i, r))
    ods = [o for o in od if o["variant"] == v and o["text_id"] == X]
    if not sel and not ods: continue
    print(f"== {v}")
    for rid, code, i, r in sel: print(f"   VCDS row {rid:6d} (RM idx {i:3d}) code {code}: {fmt(r)}")
    for o in ods: print(f"   ODIS: DID {o['did']:04X} bit {o['bit_offset']:3d} len {o['bit_length']:3d} {'s' if o['signed'] else 'u'} {'BE' if o['big_endian'] else 'LE'} {o['scaling']} x{o['factor']} +{o['offset']} {o['unit']}  ({o['name'][:30]})")
