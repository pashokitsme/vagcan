import sys, os, collections, re
sys.path.insert(0, os.path.dirname(__file__))
from alphabet import decoder
rm = collections.defaultdict(list)
for line in open(sys.argv[1], encoding="utf-8"):
    k, i, p = line.rstrip("\n").split("\t", 2); rm[int(k)].append(p)
raw = open(sys.argv[2], "rb").read().decode("latin-1")
rows = [r for r in raw.split("\r\n") if r]
st = collections.Counter(); ex = []
for r in rows:
    k = int(r[:6]); code = decoder(k)(r[7:])
    if k not in rm: st['not_in_RM'] += 1; continue
    st['in_RM'] += 1
    n = len(rm[k]); v = int(code, 16)
    st['code<rows' if v < n else 'code>=rows'] += 1
    ex.append((k, code, v, n))
print(st)
for e in ex[:25]: print(e, rm[e[0]][e[2]] if e[2] < e[3] else '-')
