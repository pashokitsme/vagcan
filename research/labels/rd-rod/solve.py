"""Break one `RD.rod [DTC]` table's digit substitution using `Codes.dat` as the crib.

The ordering attack (`crates/vag-data/src/glyphs.rs`) needs a table to pin all ten
glyphs by itself, which takes ~10 rows; the reference car's tables have 1–16. This
solver uses the far half of the chain instead: **every field-0 value must be a key of
`Codes.dat`**, a 34 716-element set. A table with a few distinct field-0 values is then
over-determined, and the row-ordering constraints prune what is left.

Field 0 is `<component>`; the row is `f0 <sep> f1(2-char code) <sep> f2 <sep> …`, seven
fields. Only field 0 is used — field 2 lives in a different id space
(`research/car/whole-car-survey.md` §3).

Two escapes are deliberate and both are reported rather than hidden:

* `misses` — `codes-dat.md` §5 measured 47 of 48 field-0 values present, so insisting on
  100 % would refuse a table for one stale row. A solution is scored by how many values
  it fails to place, and only the best score survives.
* `exhausted` — a node budget that is hit looks exactly like "no solution" unless it is
  reported. It was, and that is why `000531` first read as a refutation.
"""

import collections

import tables


def field0s(rows, sep):
    """Distinct field-0 strings of a table, longest first (most constraining)."""
    vals = {r.split(sep)[0] for r in rows}
    return sorted((v for v in vals if v), key=lambda v: (-len(v), v))


def by_length(keys):
    out = collections.defaultdict(set)
    for k in keys:
        out[len(str(k))].add(str(k))
    return {k: sorted(v) for k, v in out.items()}


class Solver:
    def __init__(self, keylen, cons=(), misses=0, budget=3_000_000):
        self.keylen = keylen
        self.cons = list(cons)
        self.misses = misses
        self.budget = budget

    def solve(self, f0s):
        """(solutions, exhausted). `solutions` is deduplicated on the map."""
        self.steps = 0
        self.out = {}
        self.best = self.misses + 1
        self._rec(f0s, 0, {}, set(), 0)
        return list(self.out.values()), self.steps <= self.budget

    def _ok(self, m):
        return all(m[a] < m[b] for a, b in self.cons if a in m and b in m)

    def _rec(self, f0s, i, m, used, missed):
        self.steps += 1
        if self.steps > self.budget:
            return
        if missed > self.best:
            return
        if i == len(f0s):
            if missed < self.best:
                self.best, self.out = missed, {}
            if missed == self.best:
                self.out["".join(f"{g}{d}" for g, d in sorted(m.items()))] = (dict(m), missed)
            return
        s = f0s[i]
        for cand in self.keylen.get(len(s), ()):
            m2, u2 = dict(m), set(used)
            for g, d in zip(s, cand):
                if g in m2:
                    if m2[g] != d:
                        break
                elif d in u2:
                    break
                else:
                    m2[g] = d
                    u2.add(d)
            else:
                if self._ok(m2):
                    self._rec(f0s, i + 1, m2, u2, missed)
            if self.steps > self.budget:
                return
        # this value is allowed not to be a key at all
        if missed + 1 <= self.best:
            self._rec(f0s, i + 1, m, used, missed + 1)


def decode_with(s, m):
    if any(g not in m for g in s):
        return None
    return int("".join(m[g] for g in s))


def separator(rows):
    """The glyph occurring exactly 6 times in every row, if there is exactly one."""
    cands = None
    for r in rows:
        c = collections.Counter(r)
        here = {g for g, n in c.items() if n == 6 and g in tables.GLYPHS}
        cands = here if cands is None else (cands & here)
        if not cands:
            return None
    return next(iter(cands)) if len(cands) == 1 else None
