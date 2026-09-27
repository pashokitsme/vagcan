"""Decode a global registry section (RM.rod [MWB] etc.): each row `KKKKKK,<payload>` under srand(K).
OUT: TSV key \t row-index-within-key \t decoded payload.  Prints field statistics."""
import sys, os, collections, re
sys.path.insert(0, os.path.dirname(__file__))
from alphabet import decoder
src, out = sys.argv[1], sys.argv[2]
raw = open(src, "rb").read().decode("latin-1")
rows = [r for r in raw.split("\r\n") if r]
nf = collections.Counter(); perkey = collections.Counter(); bad = 0
dec_cache = {}
with open(out, "w", encoding="utf-8") as fo:
    last = None; idx = 0
    for r in rows:
        if not (len(r) > 7 and r[:6].isdigit() and r[6] == ','):
            bad += 1; continue
        k = int(r[:6])
        if k != last: idx = 0; last = k
        else: idx += 1
        if k not in dec_cache: dec_cache = {k: decoder(k)}
        p = dec_cache[k](r[7:])
        fo.write(f"{k}\t{idx}\t{p}\n")
        nf[p.count(',') + 1] += 1
        perkey[k] += 1
print("rows", len(rows), "bad", bad, "keys", len(perkey))
print("field counts", nf.most_common(8))
dist = collections.Counter(perkey.values())
print("rows per key", sorted(dist.items())[:30], "max", max(perkey.values()))
print("key range", min(perkey), max(perkey))
