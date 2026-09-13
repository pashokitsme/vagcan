"""`RD.rod [DTC]` as tables, plus the ordering attack of `crates/vag-data/src/glyphs.rs`.

A row is `<6- or 8-digit plaintext key>,<payload>`; rows sharing a key are one
table. Inside a table the payload is written in a substitution alphabet and the
rows are sorted on their plaintext, which is what the ordering attack uses.
"""

from collections import defaultdict

GLYPHS = set("0123456789.-_,")


def load(path="/tmp/rd_dtc.txt"):
    """[(key, payload)] in file order."""
    rows = []
    with open(path, "rb") as f:
        for line in f.read().split(b"\r\n"):
            if not line:
                continue
            k, _, rest = line.partition(b",")
            rows.append((k.decode("latin-1"), rest.decode("latin-1")))
    return rows


def tables(rows):
    """{key: [payload]} preserving file order."""
    out = defaultdict(list)
    for k, p in rows:
        out[k].append(p)
    return out


def constraints(rows, ignore=()):
    """Ordering constraints (smaller, larger) from consecutive sorted rows."""
    out = []
    for a, b in zip(rows, rows[1:]):
        pair = next(((x, y) for x, y in zip(a, b) if x != y), None)
        if pair is None:
            continue
        x, y = pair
        if x in ignore or y in ignore or x not in GLYPHS or y not in GLYPHS:
            continue
        out.append((x, y))
    return out


def total_order(cons):
    """Kahn's algorithm, refusing anything but one total order over 10 glyphs."""
    seen = {g for c in cons for g in c}
    if len(seen) != 10:
        return None
    greater = defaultdict(set)
    for x, y in cons:
        greater[x].add(y)
    incoming = {g: 0 for g in seen}
    for x, ys in greater.items():
        for y in ys:
            incoming[y] += 1
    ready = [g for g, n in incoming.items() if n == 0]
    order = []
    while ready:
        if len(ready) > 1:
            return None
        g = ready.pop()
        order.append(g)
        for y in sorted(greater.get(g, ())):
            incoming[y] -= 1
            if incoming[y] == 0:
                ready.append(y)
    return "".join(order) if len(order) == 10 else None


def decode(text, alphabet):
    """Read an enciphered field as an integer, or None if a glyph is unknown."""
    v = 0
    for ch in text:
        i = alphabet.find(ch)
        if i < 0:
            return None
        v = v * 10 + i
    return v if text else None
