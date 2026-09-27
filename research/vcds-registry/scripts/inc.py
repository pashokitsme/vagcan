"""Read the rows of an uncompressed (TEA) section, recovering the first row, whose key digits 3..5
are destroyed by the block-0 IV, by trying the 1000 keys consistent with the surviving digits and
keeping only decodes that land on an existing file stem (INC) — a closed-list check."""
import os, sys, re
sys.path.insert(0, os.path.dirname(__file__))
import rodlib
from alphabet import decoder
UDS = os.path.expanduser("~/vcds-en/UDS_EV/")
STEMS = {f[:-4] for f in os.listdir(UDS) if f.lower().endswith(".rod")}

def rows_of_text(txt, closed=None):
    """txt: latin-1 plaintext of a section whose first 8 bytes may be wrong. Returns [(key, payload)],
    and for row 0 the list of candidate (key, payload) that pass `closed` (a predicate)."""
    rows = txt.split("\r\n")
    out = []
    first = rows[0] if rows else ""
    cands = []
    if len(first) > 8:
        pre = first[:3]
        body = first[8:]           # exact from byte 8 on
        if pre.isdigit():
            for x in range(1000):
                k = int(pre + "%03d" % x)
                dec = decoder(k)(body)
                if closed is None or closed(dec):
                    cands.append((k, dec))
    for r in rows[1:]:
        if len(r) > 7 and r[:6].isdigit() and r[6] == ",":
            out.append((int(r[:6]), decoder(int(r[:6]))(r[7:])))
        elif r:
            out.append((None, r))
    return cands, out

def inc_closed(dec):
    # payload from byte 8 = file name minus its first character
    return any(s[1:] == dec for s in STEMS if len(s) == len(dec) + 1)

def read_inc(fname, text=None):
    """INC rows of a file: from a given decoded plaintext (cracked), or from the TEA section itself."""
    if text is not None:
        rows = [r for r in text.split("\r\n") if r]
        return [decoder(int(r[:6]))(r[7:]) for r in rows if r[:6].isdigit()], "cracked"
    d = open(UDS + fname, "rb").read()
    for tag, comp, plain, ci in rodlib.sections(d):
        if tag != "INC": continue
        kind, D = rodlib.classify(tag, comp, plain, ci)
        if kind != "tea": return None, kind
        txt = rodlib.tea_text(tag, plain, ci).decode("latin-1")
        cands, rest = rows_of_text(txt, inc_closed)
        names = []
        firsts = sorted({ [s for s in STEMS if s[1:] == dec][0] for k, dec in cands })
        if len(firsts) == 1: names.append(firsts[0])
        elif len(firsts) > 1: names.append("AMBIGUOUS:" + "|".join(firsts))
        else: names.append("UNRESOLVED")
        names += [p for k, p in rest]
        return names, "tea"
    return [], "absent"

if __name__ == "__main__":
    for f in sys.argv[1:]:
        print(f, read_inc(f))
