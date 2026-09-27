"""Shared loaders: decoded TTTEXT, the RM registry, ODIS readings."""
import os, collections, sqlite3
S = os.environ.get("ODIS_CRIB") or os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "scratch")
KIND_PREFIX = {"2": "IDE", "7": "MAS"}

def load_tttext():
    tt = {}
    for line in open(os.path.join(S, "out/tttext_plain.tsv"), encoding="utf-8"):
        i, t = line.rstrip("\n").split("\t", 1); tt[int(i)] = t
    return tt

def tail(t):
    """(name, kind, number-string) or (name, None, None)."""
    p = t.split(",")
    if len(p) >= 3: return ",".join(p[:-2]), p[-2], p[-1]
    return p[0], None, None

def odx_id(t):
    """TTTEXT record -> 'IDE00022' / 'MAS00362' when its tail says so."""
    name, kind, num = tail(t)
    if kind in KIND_PREFIX and num and num.isdigit():
        return KIND_PREFIX[kind] + num
    return None

def num(x):
    if x in ("", None): return None
    try: return float(x)
    except ValueError: return None

def parse_rm(p):
    f = p.split(",")
    if len(f) == 13: h6, h4 = f[11], f[12]
    elif len(f) == 12: h6, h4 = None, f[11]
    else: return None
    return {
        "did": int(f[0]) if f[0].isdigit() else None, "f1": f[1], "f2": f[2],
        "off": num(f[3]), "num": num(f[4]), "den": num(f[5]), "unit": f[6],
        "byte": int(f[7]) if f[7].isdigit() else None, "bit": int(f[8]) if f[8].isdigit() else None,
        "len": int(f[9]) if f[9].isdigit() else None, "name": f[10], "h6": h6, "h4": h4, "raw": p,
    }

def load_rm(path=None):
    """list of (key, idx, parsed) in file order (row number = list index + 1)."""
    rows = []
    for line in open(path or os.path.join(S, "out/rm_mwb.tsv"), encoding="utf-8"):
        k, i, p = line.rstrip("\n").split("\t", 2)
        rows.append((int(k), int(i), parse_rm(p)))
    return rows

def load_odis():
    db = sqlite3.connect(os.path.expanduser("~/.vagcan/data/SK37X/cache.sqlite"))
    q = "select id, variant, did, name, unit, bit_offset, bit_length, signed, big_endian, text_id, scaling, factor, offset from reading"
    cols = ["id", "variant", "did", "name", "unit", "bit_offset", "bit_length", "signed", "big_endian", "text_id", "scaling", "factor", "offset"]
    return [dict(zip(cols, r)) for r in db.execute(q)]

_kind_cache = {}
def mwb_kind(path):
    """regime of the MWB section a dump came from ('tea' | 'classic' | ...)."""
    import rodlib
    f = os.path.basename(os.path.dirname(path)) + ".rod"
    if f not in _kind_cache:
        d = open(os.path.expanduser("~/vcds-en/UDS_EV/") + f, "rb").read()
        _kind_cache[f] = {t: rodlib.classify(t, c, p, ci)[0] for t, c, p, ci in rodlib.sections(d)}.get("MWB")
    return _kind_cache[f]

def read_unit_mwb(path):
    """[(row id, decoded code)] of a dumped MWB section. The first row of an uncompressed (TEA) section
    is dropped: its block-0 bytes 3..7 are wrong when product != 0 and the dump does not repair them."""
    from alphabet import decoder
    lines = open(path, "rb").read().decode("latin-1").split("\r\n")
    if mwb_kind(path) == "tea": lines = lines[1:]
    out = []
    for line in lines:
        if len(line) == 9 and line[:6].isdigit() and line[6] == ",":
            rid = int(line[:6]); out.append((rid, decoder(rid)(line[7:])))
    return out
