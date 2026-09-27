"""Resolve a unit's section ids as 1-based row numbers into a registry TSV (key, idx, payload)."""
import sys, os, collections, re, json
sys.path.insert(0, os.path.dirname(__file__))
from alphabet import decoder
reg = []
for line in open(sys.argv[1], encoding="utf-8"):
    k, i, p = line.rstrip("\n").split("\t", 2); reg.append((int(k), int(i), p))
raw = open(sys.argv[2], "rb").read().decode("latin-1")
rows = [r for r in raw.split("\r\n") if r]
proven = set()
if len(sys.argv) > 3:
    for d in json.load(open(sys.argv[3]))["defs"]:
        proven.add("%04X" % d["address"]["Uds"])
print("proven DIDs:", sorted(proven))
hit = collections.Counter()
for r in rows:
    uid = int(r[:6]); code = decoder(uid)(r[7:])
    if uid - 1 >= len(reg): hit['out_of_range'] += 1; continue
    k, i, p = reg[uid - 1]
    f = p.split(',')
    f11 = f[11] if len(f) > 11 else ''
    did = f11[2:] if len(f11) == 6 else ''
    if did in proven:
        hit['proven'] += 1
        print("PROVEN", uid, code, "-> RMrow key", k, "idx", i, "|", p)
print(hit)
for r in rows[:15]:
    uid = int(r[:6]); code = decoder(uid)(r[7:]); k, i, p = reg[uid - 1]
    print(uid, code, "-> key", k, "idx", i, "|", p)
