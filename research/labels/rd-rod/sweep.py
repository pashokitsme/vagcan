"""Step 1: is the substitution alphabet a property of the table, or of something larger?

Solves every table of `RD.rod [DTC]` that the ordering attack can solve, then counts
distinct alphabets. If the alphabet were file-wide there would be one; if it is keyed by
something coarser than the table there would be few.
"""

import collections
import sys

import tables
from solve import separator


def main():
    rows = tables.load()
    T = tables.tables(rows)
    seps = {}
    for k, rs in T.items():
        s = separator(rs)
        if s:
            seps[k] = s
    print(f"tables {len(T)}, with a unique 6-count separator {len(seps)}", flush=True)

    solved = {}
    partial = collections.Counter()
    for i, (k, rs) in enumerate(T.items()):
        if len(rs) < 5 or k not in seps:
            continue
        cons = tables.constraints(rs, ignore=(seps[k],))
        a = tables.total_order(cons)
        if a:
            solved[k] = a
        else:
            partial[len({g for c in cons for g in c})] += 1
        if i % 20000 == 0:
            print(f"  ..{i}", flush=True)

    print(f"solved {len(solved)} tables", flush=True)
    print(f"unsolved glyph-count hist {sorted(partial.items())}", flush=True)
    counts = collections.Counter(solved.values())
    print(f"distinct alphabets {len(counts)}")
    for a, n in counts.most_common(15):
        print(f"  {a}  x{n}")
    with open("/tmp/solved_alphabets.txt", "w") as f:
        for k, a in solved.items():
            f.write(f"{k}\t{seps[k]}\t{a}\n")


if __name__ == "__main__":
    sys.exit(main())
