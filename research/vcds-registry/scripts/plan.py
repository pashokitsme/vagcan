"""For each matched (ODIS variant -> VCDS file): what it takes to get the unit's full MWB list.
status per file: sections by regime; INC names when readable without a key."""
import os, sys, json, collections
sys.path.insert(0, os.path.dirname(__file__))
import rodlib, inc
UDS = inc.UDS
match = json.load(open(os.path.join(sys.argv[1], "match.json")))
cache = json.load(open(os.path.join(sys.argv[1], "..", "ivcache.json")))
cached_files = {k.split("\t")[0] for k in cache}
def sec(f):
    d = open(UDS + f, "rb").read()
    return {t: rodlib.classify(t, c, p, ci)[0] for t, c, p, ci in rodlib.sections(d)}
plan = {}
need = collections.Counter(); why = collections.Counter()
for v, m in sorted(match.items()):
    f = m["file"]
    if not f: continue
    s = sec(f)
    own = s.get("MWB", "absent")
    incs, inck = inc.read_inc(f)
    mwb_sources = []
    blockers = []
    if own != "absent": mwb_sources.append((f, own))
    if inck == "tea":
        for n in incs or []:
            if n.startswith(("UNRESOLVED", "AMBIGUOUS")): blockers.append(("inc-first-row", n)); continue
            t = sec(n + ".rod") if os.path.exists(UDS + n + ".rod") else {}
            if "MWB" in t: mwb_sources.append((n + ".rod", t["MWB"]))
    elif inck in ("classic",):
        blockers.append(("inc-classic", f))
    elif inck == "shifted":
        blockers.append(("inc-shifted", f))
    cracks = set()
    ok = True
    for fn, k in mwb_sources:
        if k == "shifted": ok = False; blockers.append(("mwb-shifted", fn))
        elif k == "classic" and fn not in cached_files: cracks.add(fn)
    for b, fn in blockers:
        if b == "inc-classic" and fn not in cached_files: cracks.add(fn)
        if b in ("inc-shifted",): ok = False
    plan[v] = {"file": f, "own_mwb": own, "inc": incs, "inc_kind": inck, "mwb_sources": mwb_sources, "blockers": blockers, "cracks": sorted(cracks), "reachable": ok}
    need["reachable (no shifted on path)" if ok else "blocked by shifted"] += 1
json.dump(plan, open(os.path.join(sys.argv[1], "plan.json"), "w"), indent=1)
print(dict(need))
allc = collections.Counter(c for p in plan.values() if p["reachable"] for c in p["cracks"])
print("distinct files to crack for all reachable variants:", len(allc))
