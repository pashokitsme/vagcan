"""The row selector: a control unit's own `.rod` says which `RD.rod` row is its fault.

`research/fault-naming-hop.md` §5.2 left the chain with one row too many — table
`000297` has 36 rows and nothing inside `RD.rod` picks between them. The selector is
external and it is the unit's own ODX file:

    a unit .rod `[DTC]` row is `<index>,<2-char code>`
    and `<index>` is the **1-based row number in `RD.rod [DTC]`**

so a fault the unit can report names exactly one of that table's rows. See §10 of the
writeup for the evidence and for the refutation of the "the ids are `RD.rod` table keys"
reading that §8.1 proposed.

Needs `/tmp/rd_dtc.txt` (see `verify.py`'s docstring) and a decoded unit `[DTC]`
section; the IV tails recovered so far are listed in §10.4.

Research tooling only. `crates/vag-data/src/rod.rs` and `codes.rs` remain authoritative.
"""

import codes
import solve
import tables

ROWS = tables.load()
T = tables.tables(ROWS)
EN = codes.load_en()
RU = codes.load_ru()
KEYLEN = solve.by_length(EN.keys())


def unit_index(path):
    """{`RD.rod` table key: (selected row payload, the unit's own 2-char code)}.

    The key is the raw 24-bit DTC in decimal, so this is directly the catalogue of
    faults the unit can report — and the payload is *the* row, not a candidate set.
    """
    out = {}
    for line in open(path, "rb").read().decode("latin-1").split("\r\n"):
        if not line:
            continue
        idx, _, code = line.partition(",")
        key, payload = ROWS[int(idx) - 1]
        out[key] = (payload, code)
    return out


def _ftb_consistent(rows, sep, m):
    """Reject an alphabet whose 8-digit rows disagree with their own `f1`.

    An 8-digit `Codes.dat` key is `(letter << 20) | (code << 8) | ftb` — 9 529 586 is
    `0x9168F2` and VCDS prints `B1168 F2` — so such a row pins its own failure-type
    field: digits through the table alphabet, `A-F` through a per-table letter
    substitution that has to stay injective across the table.

    Measured: it prunes nothing on the reference car's tables, because none of their
    rows has an 8-digit `f0`. Kept because it costs nothing and it is a real invariant.
    """
    letters, used = {}, {}
    for r in rows:
        f = r.split(sep)
        if len(f) < 2 or not f[0] or len(f[1]) != 2:
            continue
        v = solve.decode_with(f[0], m)
        if v is None or v < 10**7:
            continue
        want = f"{v & 0xFF:02X}"
        for c, w in zip(f[1], want):
            if c in m:
                if m[c] != w:
                    return False
            elif c.isalpha():
                if letters.setdefault(c, w) != w or used.setdefault(w, c) != c:
                    return False
            else:
                return False
    return True


def name(key, selected, budget=4_000_000, misses=1, ftb_filter=True):
    """Candidate `(Codes.dat key, failure type)` for one **selected** row.

    The selection is what the unit file supplies; what is left is the per-table
    substitution (§5.1), which is unaffected by it. Returns `(set|None, note)`; a set
    of one is an answer, anything else must be reported as a number and no name.
    """
    rows = T.get(key)
    if not rows:
        return None, "no table"
    sep = solve.separator(rows)
    if not sep:
        return None, "no separator"
    cons = tables.constraints(rows, ignore=(sep,))
    sols, exhausted = solve.Solver(KEYLEN, cons, misses=misses, budget=budget).solve(
        solve.field0s(rows, sep)
    )
    if ftb_filter:
        sols = [s for s in sols if _ftb_consistent(rows, sep, s[0])]
    if not sols:
        return None, f"no alphabet (exhausted={exhausted})"
    f = selected.split(sep)
    vals = set()
    for m, _ in sols:
        v = solve.decode_with(f[0], m)
        if v is not None:
            vals.add((v, "".join(m.get(g, g) for g in f[1]) if len(f) > 1 else ""))
    return vals, f"{len(sols)} alphabets, exhausted={exhausted}"
