"""Global test: for each ODIS (IDE/MAS id, DID, layout, scaling), is there an RM row under a key whose
TTTEXT tail names that id with the same DID / length / factor?  Control: the same test against the RM rows
of a different id drawn at random (seeded)."""
import sys, os, collections, random, json
sys.path.insert(0, os.path.dirname(__file__))
from common import *
tt = load_tttext(); rm = load_rm(); od = load_odis()
by_id = collections.defaultdict(list)          # 'IDE00022' -> RM rows
keys_for = collections.defaultdict(set)
for k, i, r in rm:
    if r is None: continue
    t = tt.get(k)
    x = odx_id(t) if t else None
    if x: by_id[x].append(r); keys_for[x].add(k)
print("ODX ids with RM rows:", len(by_id), " ids with >1 RM key:", sum(1 for v in keys_for.values() if len(v) > 1))
ids = sorted(by_id)
rng = random.Random(20260928)
def fac(r):
    if r["num"] is None or r["den"] in (None, 0): return None
    return r["num"] / r["den"]
def close(a, b): return a is not None and b is not None and abs(a - b) <= 1e-6 * max(1.0, abs(a), abs(b))
st = collections.Counter()
for o in od:
    x = o["text_id"]
    if not x or x[:3] not in ("IDE", "MAS"): continue
    st["odis rows with IDE/MAS"] += 1
    cand = by_id.get(x)
    if not cand: st["no RM rows for this id"] += 1; continue
    st["id present in RM"] += 1
    ctrl = by_id[ids[rng.randrange(len(ids))]]
    for tag, rows in (("real", cand), ("ctrl", ctrl)):
        d = [r for r in rows if r["did"] == o["did"]]
        if d: st[tag + " DID"] += 1
        dl = [r for r in d if r["len"] == o["bit_length"]]
        if dl: st[tag + " DID+len"] += 1
        if o["scaling"] == "linear":
            st[tag + " linear rows tested"] += 1
            dlf = [r for r in dl if close(fac(r) if fac(r) is not None else (1.0 if r["num"] is None and r["den"] is None else None), o["factor"])]
            if dlf: st[tag + " DID+len+factor"] += 1
for k, v in sorted(st.items()): print(f"  {k:40s} {v}")
