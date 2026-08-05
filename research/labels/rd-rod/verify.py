"""Reproduce `research/labels/fault-naming-hop.md` §4: the chain against VCDS's own answers.

For every crib pair in `pairs.tsv`, look up the raw number's `RD.rod [DTC]` table, break
its substitution against `Codes.dat`, and check whether a row's field 0 lands on the text
VCDS printed. Prints the 10 / 0 / 28 of §4 and §5.

    python3 verify.py        # needs /tmp/rd_dtc.txt — see the module docstring of rod.py

The `[DTC]` payload is produced by:

    import rod
    data = open('~/vcds-en/UDS_EV/RD.rod', 'rb').read()
    secs = dict(rod._sections_raw(data))
    open('/tmp/rd_dtc.txt', 'wb').write(
        rod.decode_section(b'DTC', secs[b'DTC'], bytes([0x5c, 0xb0, 0x48, 0xd4, 0x3f])))
"""

import csv
import sys

import codes
import solve
import tables


def load_pairs(path="pairs.tsv"):
    out = []
    with open(path, encoding="utf-8") as f:
        for row in csv.reader((l for l in f if not l.startswith("#")), delimiter="\t"):
            out.append((int(row[0]), row[2], row[3], row[4]))
    return sorted(set(out))


def main():
    T = tables.tables(tables.load())
    en, ru = codes.load_en(), codes.load_ru()
    keylen = solve.by_length(en.keys())
    matched = wrong = ambiguous = 0

    for raw, sae, ftb, text in load_pairs():
        key = f"{raw:06d}" if raw < 10**6 else f"{raw:08d}"
        rows = T.get(key)
        sep = solve.separator(rows) if rows else None
        if not sep:
            ambiguous += 1
            print(f"{raw:>9} {sae} {ftb}  no table / no separator")
            continue
        f0 = solve.field0s(rows, sep)
        cons = tables.constraints(rows, ignore=(sep,))
        sols, exhausted = solve.Solver(keylen, cons, misses=1).solve(f0)
        if len(sols) != 1:
            ambiguous += 1
            print(f"{raw:>9} {sae} {ftb}  {len(rows):>3}r {len(f0):>3}f0  "
                  f"{len(sols)} alphabets (exhausted={exhausted})")
            continue
        m = sols[0][0]
        hit = None
        for r in rows:
            f = r.split(sep)
            v = solve.decode_with(f[0], m) if f[0] else None
            if v is None:
                continue
            txt = ru.get(v, "")
            if txt and txt.split(": ")[0].strip() == text.split(": ")[0].strip():
                hit = (v, "".join(m.get(g, g) for g in f[1]))
                break
        if hit:
            matched += 1
            agree = "ftb ok" if hit[1][1:] == ftb[1:] else f"ftb {hit[1]} vs {ftb}"
            print(f"{raw:>9} {sae} {ftb}  {len(rows):>3}r {len(f0):>3}f0  "
                  f"MATCH {hit[0]} {en.get(hit[0], '?')[:44]}  [{agree}]")
        else:
            wrong += 1
            print(f"{raw:>9} {sae} {ftb}  {len(rows):>3}r {len(f0):>3}f0  "
                  f"SOLVED BUT NO TEXT MATCH — want {text[:40]}")

    print(f"\nmatched {matched}, wrong {wrong}, ambiguous {ambiguous}")
    return 0 if wrong == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
