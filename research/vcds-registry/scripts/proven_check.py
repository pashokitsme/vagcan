"""Every car-proven row of a unit vs the RM rows the unit's own MWB selects (id = 1-based RM row)."""
import sys, os, collections, json
sys.path.insert(0, os.path.dirname(__file__))
from alphabet import decoder
reg = []
for line in open(sys.argv[1], encoding="utf-8"):
    k, i, p = line.rstrip("\n").split("\t", 2); reg.append((int(k), int(i), p))
tt = {}
for line in open(sys.argv[2], encoding="utf-8"):
    i, t = line.rstrip("\n").split("\t", 1); tt[int(i)] = t
raw = open(sys.argv[3], "rb").read().decode("latin-1")
rows = [r for r in raw.split("\r\n") if r]
sel = collections.defaultdict(list)
for r in rows:
    uid = int(r[:6]); code = decoder(uid)(r[7:])
    k, i, p = reg[uid - 1]; f = p.split(',')
    if f[0].isdigit(): sel[int(f[0])].append((uid, code, k, i, p))
defs = json.load(open(sys.argv[4]))["defs"]
ok = 0
for d in defs:
    did = d["address"]["Uds"]; sc = d["scaling"]
    got = sel.get(did, [])
    print(f"{did:04X} {d['name'][:34]:34} {d['raw_form']:8} {json.dumps(sc)[:60]}")
    for uid, code, k, i, p in got:
        print(f"     unit row {uid} code {code} -> RM key {k} [{tt.get(k,'')[:45]}] idx {i}: {p}")
    ok += bool(got)
print(f"proven DIDs found among the unit's selected RM rows: {ok}/{len(defs)}; distinct DIDs selected by the unit: {len(sel)}")
