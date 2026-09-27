import sys, os, collections
sys.path.insert(0, os.path.dirname(__file__))
from alphabet import decoder
tt = {}
for line in open(sys.argv[1], encoding="utf-8"):
    i, t = line.rstrip("\n").split("\t", 1); tt[int(i)] = t
for d in sys.argv[2:]:
    for tag in sorted(os.listdir(d)):
        raw = open(os.path.join(d, tag), "rb").read().decode("latin-1")
        rows = [r for r in raw.split("\r\n") if r]
        print(f"== {os.path.basename(d)} [{tag}] rows={len(rows)}")
        dec_codes = collections.Counter()
        for r in rows[:12]:
            if len(r) > 7 and r[:6].isdigit():
                k = int(r[:6]); body = r[7:] if r[6] == ',' else r[6:]
                print("   ", repr(r[:70]), "-> key", k, repr(decoder(k)(r[6:])[:70]), "| TT:", tt.get(k, "")[:60])
            else:
                print("   RAW", repr(r[:80]))
        for r in rows:
            if len(r) > 7 and r[:6].isdigit():
                dec_codes[decoder(int(r[:6]))(r[6:])] += 1
        print("   decoded-code histogram:", dec_codes.most_common(12), "distinct", len(dec_codes))
