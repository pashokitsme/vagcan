import sys, os, re, collections
sys.path.insert(0, os.path.dirname(__file__))
from alphabet import decoder
tt = {}
for line in open(sys.argv[1], encoding="utf-8"):
    i, t = line.rstrip("\n").split("\t", 1); tt[int(i)] = t
raw = open(sys.argv[2], "rb").read().decode("latin-1")
rows = [r for r in raw.split("\r\n") if r]
ids = []
stat = collections.Counter()
tail_kind = collections.Counter()
for r in rows:
    if not (len(r) >= 9 and r[:6].isdigit() and r[6] == ','):
        stat['malformed'] += 1; continue
    k = int(r[:6]); code = decoder(k)(r[7:])
    t = tt.get(k)
    stat['in_tttext' if t is not None else 'not_in_tttext'] += 1
    stat['hexcode' if re.fullmatch(r'[0-9A-F]{2}', code) else 'nonhex'] += 1
    if t is not None:
        parts = t.split(',')
        tail_kind[parts[1] if len(parts) >= 3 else '-'] += 1
    ids.append((k, r[7:], code, t))
print(stat, tail_kind.most_common())
want = [18605, 103074, 98287, 99967, 103124, 100415]
for k, c, d, t in ids:
    if k in want: print('WANT', k, c, d, t)
for k, c, d, t in ids[:40]:
    print(k, c, d, (t or '')[:90])
