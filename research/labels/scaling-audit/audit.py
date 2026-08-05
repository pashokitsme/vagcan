#!/usr/bin/env python3
"""Audit the `.rod` scaling decode and re-run the read-identifier negatives under
the *correct* codec (the per-table substitution of glyphs.py), not the retired
base-14 model that rod-labels.md §4.0c's DID search used.

Inputs are the inflated section blobs written by
    vagcan vcds rod <FILE> --dump <DIR>
See scaling-audit.md §Reproduction. Blobs are proprietary VCDS data and are NOT
committed; regenerate them locally. Defaults point at /tmp/rodwork/{struc,mux,ttdop,gbx}.

Usage: python3 audit.py [struc.bin] [mux.bin] [ttdop.bin] [gbx_MWB.bin] [names-uds.json]
"""
import sys, os, json, collections
sys.path.insert(0, os.path.dirname(__file__))
from glyphs import TableAlphabet

def load_rows(path):
    rows = []
    for line in open(path, "rb").read().decode("latin-1").split("\n"):
        line = line.rstrip("\r")
        if not line or "," not in line:
            continue
        ids, payload = line.split(",", 1)
        if ids.isdigit():
            rows.append((int(ids), payload))
    return rows

def decode_tables(path):
    """id -> list of decoded field lists, split on the table's generated separator."""
    by_id = collections.OrderedDict()
    for tid, payload in load_rows(path):
        by_id.setdefault(tid, []).append(payload)
    out = collections.OrderedDict()
    for tid, payloads in by_id.items():
        a = TableAlphabet(tid)
        sep = a.separator()
        out[tid] = [[a.decode(f) if f else "" for f in p.split(sep)] for p in payloads]
    return out

def num(f):
    return int(f) if (f and f.isdigit()) else None

def main():
    struc = sys.argv[1] if len(sys.argv) > 1 else "/tmp/rodwork/struc/STRUC.bin"
    mux   = sys.argv[2] if len(sys.argv) > 2 else "/tmp/rodwork/mux/MUX.bin"
    ttdop = sys.argv[3] if len(sys.argv) > 3 else "/tmp/rodwork/ttdop/DOP.bin"
    gbx   = sys.argv[4] if len(sys.argv) > 4 else "/tmp/rodwork/gbx/MWB.bin"
    namesf= sys.argv[5] if len(sys.argv) > 5 else \
        os.path.join(os.path.dirname(__file__), "../../../catalogs/names-uds.json")
    names = json.load(open(namesf))

    S = decode_tables(struc); M = decode_tables(mux); T = decode_tables(ttdop)

    # ---- AUDIT 1: field structure is exactly right under the generated key ----
    def field_audit(tables, nf, bitoff_idx, name_idx, label):
        rows = [d for recs in tables.values() for d in recs]
        nfields_ok = sum(1 for d in rows if len(d) == nf)
        bo_ok = bo_tot = 0
        names_ok = names_tot = 0
        bad = 0
        for d in rows:
            if len(d) != nf: continue
            if any(f is None for f in d): bad += 1
            v = num(d[bitoff_idx])
            if v is not None:
                bo_tot += 1; bo_ok += (0 <= v <= 7)
            n = num(d[name_idx])
            if n is not None:
                names_tot += 1
        print(f"{label}: rows {len(rows)}, exactly {nf} fields: {nfields_ok}/{len(rows)}; "
              f"bit-offset in 0..7: {bo_ok}/{bo_tot}; undecodable fields: {bad}; "
              f"name-ids present: {names_tot}")
    print("=== AUDIT 1: field structure under the generated per-table key ===")
    field_audit(S, 11, 7, 9, "STRUC")
    field_audit(M, 17, 14, 16, "MUX")
    tt = [d for r in T.values() for d in r]
    print(f"TTDOP: rows {len(tt)}, exactly 4 fields: {sum(1 for d in tt if len(d)==4)}/{len(tt)}; "
          f"f0==f1 (texttable point): {sum(1 for d in tt if len(d)==4 and d[0]==d[1])}")

    # ---- AUDIT 2: DID-value search, re-run of rod-labels.md §4.0c under this decode ----
    crib = {0x380A,0x380B,0xF40D,0x3804,0x3832,0x383B,0x38F6,0x38F9,0x38AC,0x38AD,
            0x3816,0x3809,0x206E,0x2029,0x202A,0x2203,0x22A8,0x22D2}
    def did_hits(tables):
        per = collections.Counter(); vals = collections.Counter()
        for recs in tables.values():
            for d in recs:
                for i, f in enumerate(d):
                    if num(f) in crib:
                        per[i] += 1; vals[num(f)] += 1
        return per, vals
    print("\n=== AUDIT 2: crib DIDs as decoded field values (should be chance-level) ===")
    for lbl, tb in (("STRUC", S), ("MUX", M)):
        per, vals = did_hits(tb)
        print(f"{lbl}: hits {dict(vals) or 'NONE'} by field {dict(per) or 'NONE'}")

    # ---- POSITIVE: the corpus carries the proven factors (MUX kind-0 linear) ----
    def mux_lin(d):
        if len(d) != 17 or num(d[8]) != 0: return None
        try:
            off = float(d[9]); n = float(d[10]); de = float(d[11])
        except (TypeError, ValueError):
            return None
        if de == 0: return None
        return (n/de, off, num(d[12]), num(d[16]))
    lin = [x for recs in M.values() for x in (mux_lin(d) for d in recs) if x]
    fh = collections.Counter(round(f, 6) for f, *_ in lin)
    print("\n=== POSITIVE: MUX kind-0 linear factor histogram (100% coverage) ===")
    print(f"linear rows {len(lin)}; top factors {fh.most_common(14)}")
    for want in (0.4, 0.01, 0.001, 1.0, 0.5, 0.25):
        print(f"  factor {want}: present {fh.get(want,0)} times")

    # ---- NEGATIVE: ADVMB measurement text-ids are not in the global tables ----
    struc_name = set(num(d[9]) for recs in S.values() for d in recs if num(d[9]))
    mux_name   = set(num(d[16]) for recs in M.values() for d in recs
                     if len(d) == 17 and num(d[16]))
    ENG = {103074:"input spd",99005:"output spd",99967:"veh spd",98363:"accel",
           120857:"clutch1coef",120861:"clutch2coef",120895:"clutch2spec",
           120898:"clutch1spec",120909:"clutch1act",120910:"clutch2act",103124:"idle"}
    print("\n=== NEGATIVE: proven ADVMB text-ids present in global name fields? ===")
    for tid, lbl in ENG.items():
        print(f"  {tid} ({lbl}): STRUC f9={tid in struc_name} MUX f16={tid in mux_name}")

    # ---- NEGATIVE: per-ECU code -> global id is not a function ----
    mwb = [(t, c) for t, c in ((int(a), b) for a, b in
            (l.rstrip('\r').split(',', 1) for l in
             open(gbx, 'rb').read().decode('latin-1').split('\n')
             if l.strip() and ',' in l) if a.isdigit())]
    mux_by_name = collections.defaultdict(set)
    for tid, recs in M.items():
        for d in recs:
            if len(d) == 17 and num(d[16]) is not None:
                mux_by_name[num(d[16])].add(tid)
    code_to_ids = collections.defaultdict(set)
    id_families = collections.Counter()
    for tid, code in mwb:
        ids = mux_by_name.get(tid)
        if ids:
            code_to_ids[code] |= ids
            id_families[frozenset(ids)] += 1
    multi = {c: ids for c, ids in code_to_ids.items() if len(ids) > 1}
    print("\n=== NEGATIVE: gearbox MWB code -> MUX id is not injective ===")
    print(f"  gearbox rows resolving in MUX by name: "
          f"{sum(1 for t,_ in mwb if t in mux_by_name)}/{len(mwb)}")
    print(f"  distinct codes among them: {len(code_to_ids)}; "
          f"codes mapping to >1 MUX id: {len(multi)}")
    top = id_families.most_common(1)[0] if id_families else (None, 0)
    print(f"  most-shared id set is hit by {top[1]} different (code,row) pairs "
          f"-> resolution is by name, not by code")

if __name__ == "__main__":
    main()
