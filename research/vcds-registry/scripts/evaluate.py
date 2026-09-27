"""Steps 1/2 evaluation over every variant whose MWB list is open.
J1  by the measurement's ODX id at DID level (RM key's TTTEXT tail)      -> tests DID, position, length, sign, order, scaling
J2  by the parameter's ODX id (f10 name's tail) together with the DID      -> tests position, length, sign, order, scaling
J3  structural, by (DID, bit position, bit length), every ODIS row incl. rows without a text id -> coverage + sign/order/scaling
Controls: A = a random other RM row under the same key (what a global id->layout table would give);
          B = the same ODX id in a variant of another family (would any unit's placement do?);
          C = J3 against the VCDS rows of a different opened unit (chance coverage).
OUT: out/bridge.jsonl (J1+J2 records), out/eval.json (all numbers)."""
import os, sys, json, collections, random
sys.path.insert(0, os.path.dirname(__file__))
from common import *
tt = load_tttext(); rm = load_rm(); od = load_odis()
srcs = json.load(open(os.path.join(S, "out/variant_sources.json")))
odv = collections.defaultdict(list)
for o in od: odv[o["variant"]].append(o)
rows_by_key = collections.defaultdict(list)
for n, (k, i, r) in enumerate(rm): rows_by_key[k].append(n)
def close(a, b): return a is not None and b is not None and abs(a - b) <= 1e-6 * max(1.0, abs(a), abs(b))
def f2(r): return int(r["f2"]) if r["f2"].isdigit() else None
def scale_ok(r, o):
    v = f2(r); base = (v & 63) if v is not None else None
    if o["scaling"] == "linear":
        if base == 0:
            n, d, off = r["num"], r["den"], (r["off"] or 0.0)
            if n is None or not d or not close(n / d, o["factor"]): return False
            return close(off / d, o["offset"] or 0.0) or (off == 0 and not o["offset"])
        if base in (2, 7, 8): return close(o["factor"], 1.0) and close(o["offset"] or 0.0, 0.0)
        if base == 4: return r["off"] is not None and (o["did"] >> 8) == 0xF4 and int(r["off"]) == (o["did"] & 0xFF)
        return False
    if o["scaling"] == "enum": return base in (3, 5)
    return False
def agree(r, o):
    v = f2(r)
    a = {"did": r["did"] == o["did"],
         "pos": r["byte"] is not None and r["bit"] is not None and r["byte"] * 8 + r["bit"] == o["bit_offset"],
         "len": r["len"] == o["bit_length"],
         "signed": v is not None and bool(v & 128) == bool(o["signed"]),
         "order": True if o["bit_length"] <= 8 else (v is not None and bool(v & 64) == (not o["big_endian"])),
         "scale": scale_ok(r, o)}
    a["all"] = all(a.values()); return a
def best(r, os_, fields=("did", "pos", "len", "signed", "order", "scale")):
    b = None
    for o in os_:
        A = agree(r, o); s = sum(A[f] for f in fields)
        if b is None or s > b[0]: b = (s, A, o)
    return b
def ids(r, k):
    xk = odx_id(tt[k]) if k in tt else None
    xn = odx_id(tt[int(r["name"])]) if r["name"].isdigit() and int(r["name"]) in tt else None
    return xk, xn
rng = random.Random(20260928)
fam = lambda v: v.rsplit("_", 1)[0]
vw = collections.defaultdict(lambda: collections.defaultdict(list))
for o in od:
    if o["text_id"]: vw[o["text_id"]][o["variant"]].append(o)
def sig(o): return (o["did"], o["bit_offset"], o["bit_length"], o["signed"], o["big_endian"], o["scaling"], o["factor"], o["offset"])
contested = {x for x, d in vw.items() if len({sig(o) for l in d.values() for o in l}) > 1}
def complete(v):
    s = srcs[v]
    if not s["mwb"] or len(odv.get(v, [])) < 20: return False
    if s["blocked"]: return False
    return sum(len(read_unit_mwb(p)) for p in s["mwb"]) >= 20
excluded = {v: ("no MWB opened" if not s["mwb"] else "ODIS rows < 20" if len(odv.get(v, [])) < 20 else "blocked/unresolved on the INC path: %s" % s["blocked"] if s["blocked"] else "VCDS unit list < 20 rows") for v, s in srcs.items() if not complete(v)}
variants = sorted(v for v in srcs if complete(v))
unit = {}
for v in variants:
    rows = []
    for p in srcs[v]["mwb"]:
        for rid, code in read_unit_mwb(p):
            if 1 <= rid <= len(rm) and rm[rid - 1][2] is not None: rows.append((rid, code, os.path.basename(os.path.dirname(p)) + ".rod"))
    unit[v] = rows
E = collections.defaultdict(collections.Counter)
out = open(os.path.join(S, "out/bridge.jsonl"), "w")
per_variant = {}
for v in variants:
    ods = odv[v]
    byid = collections.defaultdict(list)
    for o in ods:
        if o["text_id"]: byid[o["text_id"]].append(o)
    bystruct = collections.defaultdict(list)
    for o in ods: bystruct[(o["did"], o["bit_offset"], o["bit_length"])].append(o)
    pv = collections.Counter()
    vstruct = set()
    for rid, code, src in unit[v]:
        k, idx, r = rm[rid - 1]
        pv["rows"] += 1
        if r["did"] is not None and r["byte"] is not None and r["bit"] is not None and r["len"] is not None:
            vstruct.add((r["did"], r["byte"] * 8 + r["bit"], r["len"]))
        xk, xn = ids(r, k)
        # J1
        if xk and xk in byid:
            _, A, o = best(r, byid[xk])
            E["J1"]["n"] += 1; pv["J1"] += 1
            for f, ok in A.items(): E["J1"][f] += ok
            ctd = xk in contested
            E["J1c" if ctd else "J1u"]["n"] += 1
            for f, ok in A.items(): E["J1c" if ctd else "J1u"][f] += ok
            alts = [n for n in rows_by_key[k] if n != rid - 1]
            if alts:
                _, Aa, _ = best(rm[rng.choice(alts)][2], byid[xk]); E["J1 ctrlA"]["n"] += 1
                for f, ok in Aa.items(): E["J1 ctrlA"][f] += ok
            others = [w for w in vw[xk] if fam(w) != fam(v)]
            if others:
                _, Ab, _ = best(r, vw[xk][rng.choice(sorted(others))]); tag = "J1 ctrlB" + (" contested" if ctd else "")
                E[tag]["n"] += 1; E["J1 ctrlB"]["n"] += (1 if ctd else 0) and 0
                for f, ok in Ab.items(): E[tag][f] += ok
                if ctd:
                    pass
                else:
                    pass
            out.write(json.dumps({"join": "J1 ODX id of the measurement (DID level)", "vcds_file": src, "section": "MWB", "row": rid, "code": code, "rm_key": k, "rm_idx": idx,
                "vcds_name": tail(tt.get(k, ""))[0], "odx_id": xk, "param_id": xn, "vcds": {kk: r[kk] for kk in ("did", "byte", "bit", "len", "f1", "f2", "off", "num", "den", "unit", "name", "h6", "h4")},
                "odis_variant": v, "odis_rows": [{kk: oo[kk] for kk in ("did", "bit_offset", "bit_length", "signed", "big_endian", "scaling", "factor", "offset", "unit", "text_id", "name")} for oo in byid[xk]], "agree": A}) + "\n")
        # J2
        if xn and xn in byid:
            same = [o for o in byid[xn] if o["did"] == r["did"]]
            if same:
                _, A, o = best(r, same, fields=("pos", "len", "signed", "order", "scale"))
                E["J2"]["n"] += 1
                for f, ok in A.items(): E["J2"][f] += ok
                out.write(json.dumps({"join": "J2 ODX id of the parameter + DID", "vcds_file": src, "section": "MWB", "row": rid, "code": code, "rm_key": k, "rm_idx": idx,
                    "vcds_name": tail(tt.get(int(r["name"]), ""))[0] if r["name"].isdigit() else None, "odx_id": xn, "vcds": {kk: r[kk] for kk in ("did", "byte", "bit", "len", "f1", "f2", "off", "num", "den", "unit", "name", "h6", "h4")},
                    "odis_variant": v, "odis_rows": [{kk: oo[kk] for kk in ("did", "bit_offset", "bit_length", "signed", "big_endian", "scaling", "factor", "offset", "unit", "text_id", "name")} for oo in same], "agree": A}) + "\n")
    # J3 structural, from the ODIS side
    vdids = {s[0] for s in vstruct}
    odids = {o["did"] for o in ods}
    other = rng.choice([w for w in variants if fam(w) != fam(v)])
    ostruct = set()
    for rid, code, src in unit[other]:
        r = rm[rid - 1][2]
        if r["did"] is not None and r["byte"] is not None and r["bit"] is not None and r["len"] is not None: ostruct.add((r["did"], r["byte"] * 8 + r["bit"], r["len"]))
    vrows = collections.defaultdict(list)
    for rid, code, src in unit[v]:
        r = rm[rid - 1][2]
        if r["did"] is not None and r["byte"] is not None and r["bit"] is not None: vrows[(r["did"], r["byte"] * 8 + r["bit"], r["len"])].append(r)
    for o in ods:
        key = (o["did"], o["bit_offset"], o["bit_length"])
        E["J3"]["odis rows"] += 1
        E["J3"]["DID in unit list"] += o["did"] in vdids
        if key in vstruct:
            E["J3"]["(DID,pos,len) in unit list"] += 1
            _, A, _ = best(vrows[key][0], [o]) if len(vrows[key]) == 1 else max(((sum(agree(r, o).values()), agree(r, o), o) for r in vrows[key]), key=lambda x: x[0])
            for f in ("signed", "order", "scale"): E["J3"][f] += A[f]
            E["J3"]["sign+order+scale"] += A["signed"] and A["order"] and A["scale"]
        E["J3 ctrlC"]["odis rows"] += 1
        E["J3 ctrlC"]["(DID,pos,len) in other unit"] += key in ostruct
        E["J3 ctrlC"]["DID in other unit"] += o["did"] in {s[0] for s in ostruct}
    per_variant[v] = {"vcds rows": len(unit[v]), "vcds DIDs": len(vdids), "odis rows": len(ods), "odis DIDs": len(odids), "DIDs both": len(vdids & odids),
                      "odis DIDs covered %": round(100 * len(vdids & odids) / max(len(odids), 1), 1), "J1 joined": pv["J1"], "mwb files": sorted({s for _, _, s in unit[v]})}
out.close()
json.dump({"E": E, "per_variant": per_variant, "variants": variants, "excluded": excluded}, open(os.path.join(S, "out/eval.json"), "w"), indent=1)
print("excluded:", len(excluded), collections.Counter(x.split(":")[0] for x in excluded.values()))
print(f"variants evaluated: {len(variants)}  unit MWB rows: {sum(len(unit[v]) for v in variants)}")
F = ["did", "pos", "len", "signed", "order", "scale", "all"]
def line(tag, fields=F):
    c = E[tag]; n = c["n"]
    return f"{tag:22s} n={n:6d}  " + "  ".join(f"{f} {100*c[f]/max(n,1):5.1f}%" for f in fields)
for t in ["J1", "J1 ctrlA", "J1 ctrlB", "J1c", "J1 ctrlB contested", "J1u"]: print(line(t))
print(line("J2", ["pos", "len", "signed", "order", "scale", "all"]))
c = E["J3"]; n = c["odis rows"]; m = c["(DID,pos,len) in unit list"]
print(f"J3 ODIS rows {n}: DID in unit list {100*c['DID in unit list']/n:.1f}%  (DID,pos,len) in unit list {100*m/n:.1f}%  | of those: signed {100*c['signed']/max(m,1):.1f}% order {100*c['order']/max(m,1):.1f}% scale {100*c['scale']/max(m,1):.1f}% all three {100*c['sign+order+scale']/max(m,1):.1f}%")
c = E["J3 ctrlC"]; print(f"J3 ctrlC (other unit's list): DID {100*c['DID in other unit']/c['odis rows']:.1f}%  (DID,pos,len) {100*c['(DID,pos,len) in other unit']/c['odis rows']:.1f}%")
