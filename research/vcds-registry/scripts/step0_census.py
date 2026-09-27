"""Step 0: map ODIS variants to VCDS .rod files and census their sections by regime.
OUT: out/match.json  {variant: {"file":..., "class":..., "candidates":[...]}}
     out/census.json per matched file: sections [(tag, kind, plainlen, D)]"""
import sys, os, re, json, sqlite3, collections
sys.path.insert(0, os.path.dirname(__file__))
import rodlib
UDS = os.path.expanduser("~/vcds-en/UDS_EV/")
OUT = sys.argv[1]
files = sorted(f for f in os.listdir(UDS) if f.lower().endswith(".rod"))
stems = {f[:-4]: f for f in files}
db = sqlite3.connect(os.path.expanduser("~/.vagcan/data/SK37X/cache.sqlite"))
variants = [r[0] for r in db.execute("select distinct variant from reading order by 1")]
PLAT = re.compile(r"^[A-Z]{2}\d{2}[A-Z0-9]*$")
match = {}
cls_count = collections.Counter()
for v in variants:
    m = re.fullmatch(r"(EV_.+)_(\d{3})", v)
    if not m:
        match[v] = {"file": None, "class": "not-EV_NNN", "candidates": []}; cls_count["not-EV_NNN"] += 1; continue
    base, nnn = m.group(1), m.group(2)
    cands = []
    if v in stems: cands.append(("A_exact", v))
    for s in stems:
        if s.startswith(v + "_") and PLAT.match(s[len(v)+1:]): cands.append(("B_exact+plat", s))
    if base in stems: cands.append(("C_base", base))
    for s in stems:
        if s.startswith(base + "_") and PLAT.match(s[len(base)+1:]): cands.append(("D_base+plat", s))
    def rank(c):
        k, s = c
        suf = s.rsplit("_", 1)[-1]
        return ({"A_exact": 0, "B_exact+plat": 1, "C_base": 2, "D_base+plat": 3}[k], 0 if suf == "SK37" else (1 if suf.endswith("37") else (2 if suf.startswith("SK") else 3)), s)
    cands.sort(key=rank)
    if cands:
        match[v] = {"file": stems[cands[0][1]], "class": cands[0][0], "candidates": [c[1] for c in cands]}
        cls_count[cands[0][0]] += 1
    else:
        # looser: any stem starting with base (report only)
        loose = [s for s in stems if s.startswith(base)]
        match[v] = {"file": None, "class": "unmatched", "candidates": [], "loose": loose[:8]}
        cls_count["unmatched"] += 1
json.dump(match, open(os.path.join(OUT, "match.json"), "w"), indent=1)
print("ODIS variants", len(variants), dict(cls_count))
# census over matched files (unique)
mfiles = sorted({m["file"] for m in match.values() if m["file"]})
census = {}
tagc = collections.Counter(); kindc = collections.Counter(); mwb = collections.Counter()
for f in mfiles:
    data = open(UDS + f, "rb").read()
    secs = []
    for tag, comp, plain, cipher in rodlib.sections(data):
        kind, D = rodlib.classify(tag, comp, plain, cipher) if cipher is not None else ("bad", None)
        secs.append((tag, kind, plain, D))
        tagc[tag] += 1; kindc[(tag, kind)] += 1
    census[f] = secs
    kinds = {t: k for t, k, _, _ in secs}
    mwb[kinds.get("MWB", "absent")] += 1
json.dump(census, open(os.path.join(OUT, "census.json"), "w"), indent=0)
print("matched files", len(mfiles))
print("MWB by regime over matched files:", dict(mwb))
print("tags over matched files:", tagc.most_common())
for t in sorted(tagc):
    print("  ", t, {k: c for (tt, k), c in kindc.items() if tt == t})
