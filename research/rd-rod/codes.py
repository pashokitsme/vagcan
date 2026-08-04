"""`Codes.dat` / `Code-RUS.dat` reader — the Python twin of `crates/vag-data/src/codes.rs`.

Container and per-record IV are `research/codes-dat.md` §1–§2. Research tooling only.
"""

from pathlib import Path

from rod import KS, MT, OFF_ROD, tea_cbc_decrypt


def record_iv(key, c):
    seed = b"%08d" % key
    m = seed[5]
    s = [(seed[i] + KS[(m * (i + 2)) & 0xFF] + c) & 0xFF for i in range(8)]
    return bytes((s[i] * MT[OFF_ROD[7 - i]]) & 0xFF for i in range(8))


def records(path):
    """[(key, cipher, text_len)] by declared length — never split on newlines."""
    data = Path(path).read_bytes()
    out = []
    i = 0
    n = len(data)
    while i + 11 <= n:
        head = data[i:i + 8]
        if not head.isdigit():
            i += 1
            continue
        key = int(head)
        clen, tlen = data[i + 9], data[i + 10]
        cipher = data[i + 11:i + 11 + clen]
        if len(cipher) < clen:
            break
        out.append((key, cipher, tlen))
        i += 11 + clen + 2
    return out


def load(path, c, page):
    recs = records(path)
    return {k: tea_cbc_decrypt(cipher, record_iv(k, c))[:tlen].decode(page)
            for k, cipher, tlen in recs}


EN = "/Users/pavel.smirnov/Source/repos/vcds/research/VCDS-25.12.0/Codes.dat"
RU = "/Users/pavel.smirnov/Source/repos/vcds/research/VCDS-RUS/Code-RUS.dat"


def load_en():
    return load(EN, 0, "cp1252")


def load_ru():
    return load(RU, 208, "cp1251")
