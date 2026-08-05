"""The per-table substitution alphabet, generated from the table's key.

`research/fault-naming-hop.md` §11. Every numeric field in a `.rod` table is written
in a substitution alphabet that `§5.1` measured as per-table — 95 solved tables, 95
distinct alphabets, nothing to pool. It is not a secret: **VCDS derives it from the
table's own key with the C runtime's `rand()`**, and this is that derivation.

`VCDS-ARM.exe` 26.3, `fcn.1400e6f80`, called from `fcn.1400e1400` at `0x1400e16cc`:

    srand(key);
    for i in 0..26:  swap(letters[i], letters[rand() % 26])   # "abcdefghijklmnopqrstuvwxyz"
    for i in 0..14:  swap(digits[i],  digits[rand() % 14])    # "0123456789,.-_"

`rand`/`srand` are the MSVC pair (`seed = seed*0x343FD + 0x269EC3; return seed>>16 & 0x7fff`),
read at `0x140132528`/`0x140132568`. The two shuffles share one stream, so the digit
alphabet depends on the 26 draws the letter shuffle consumed first.

Roles come from the plaintext table's positions, not from the shuffle:

    digits[0..10]  the ten decimal digits
    digits[10]     ',' — the field separator, which is why the separator is never a digit
    digits[11..14] '.', '-', '_'

so `shuffled[d]` is the glyph a plaintext `d` is written as, and reading a row means
finding each glyph in `shuffled` and taking the plaintext at that index — which is
literally what `fcn.1400e6ea0` does (`strchr(shuffled, tolower(c))`, then index into
the plaintext alphabet, re-uppercased if the input was upper).

Research tooling only; `crates/vag-data/src/glyphs.rs` is authoritative.
"""

LETTERS = "abcdefghijklmnopqrstuvwxyz"
DIGITS = "0123456789,.-_"


class Rand:
    """The MSVC CRT `rand()`."""

    def __init__(self, seed):
        self.s = seed & 0xFFFFFFFF

    def next(self):
        self.s = (self.s * 0x343FD + 0x269EC3) & 0xFFFFFFFF
        return (self.s >> 16) & 0x7FFF


def alphabets(key):
    """(letters, digits) for a table key, each a shuffle of the plaintext alphabet."""
    r = Rand(int(key))
    letters, digits = list(LETTERS), list(DIGITS)
    for i in range(26):
        j = r.next() % 26
        letters[i], letters[j] = letters[j], letters[i]
    for i in range(14):
        j = r.next() % 14
        digits[i], digits[j] = digits[j], digits[i]
    return "".join(letters), "".join(digits)


def reader(key):
    """(separator, decode) for a table key. `decode` reads one enciphered field."""
    letters, digits = alphabets(key)
    dm = {g: str(i) for i, g in enumerate(digits[:10])}
    lm = {g: LETTERS[i].upper() for i, g in enumerate(letters)}

    def decode(field):
        out = ""
        for c in field:
            if c in dm:
                out += dm[c]
            elif c.lower() in lm:
                out += lm[c.lower()]
            else:
                return None
        return out or None

    return digits[10], decode
