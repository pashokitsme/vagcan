"""Port of glyphs.rs TableAlphabet::for_key (srand(key) + two Fisher-Yates shuffles sharing one stream)."""
PLAIN_LETTERS = "abcdefghijklmnopqrstuvwxyz"
PLAIN_DIGITS = "0123456789,.-_"

def for_key(key):
    s = key & 0xFFFFFFFF
    def nxt():
        nonlocal s
        s = (s * 0x343FD + 0x269EC3) & 0xFFFFFFFF
        return (s >> 16) & 0x7FFF
    L = list(PLAIN_LETTERS); D = list(PLAIN_DIGITS)
    for i in range(26):
        j = nxt() % 26; L[i], L[j] = L[j], L[i]
    for i in range(14):
        j = nxt() % 14; D[i], D[j] = D[j], D[i]
    return "".join(L), "".join(D)

def decoder(key):
    L, D = for_key(key)
    lmap = {c: PLAIN_LETTERS[i] for i, c in enumerate(L)}
    dmap = {c: PLAIN_DIGITS[i] for i, c in enumerate(D)}
    def dec(s):
        out = []
        for ch in s:
            lo = ch.lower()
            if ch in dmap:
                out.append(dmap[ch])
            elif lo in lmap and ch.isascii() and ch.isalpha():
                p = lmap[lo]
                out.append(p.upper() if ch.isupper() else p)
            else:
                out.append(ch)
        return "".join(out)
    return dec

def load_records(path):
    recs = {}
    for line in open(path, "rb").read().decode("latin-1").split("\r\n"):
        if len(line) > 7 and line[:6].isdigit() and line[6] == ",":
            recs[int(line[:6])] = line[7:]
    return recs

if __name__ == "__main__":
    import sys
    recs = load_records(sys.argv[1])
    print(len(recs))
    for rid in [103074, 99967, 103124, 100415, 43891, 44248, 12389, 445905, 80, 3, 4]:
        ct = recs.get(rid)
        if ct is None:
            print(rid, "MISSING"); continue
        print(rid, repr(ct[:80]), "->", repr(decoder(rid)(ct)[:80]))
