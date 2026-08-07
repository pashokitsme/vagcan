#!/usr/bin/env python3
"""Match TTTEXT's enciphered records against ODIS plaintext as a closed list.

FOR: recovering names the dictionary attack cannot reach, by treating VW's own
ODIS strings as the answer set rather than as vocabulary. Findings and the
measured precision are in `research/labels/odis-crib.md`; it produced 18,842
readings at >= 12 letters on project SK37X.

IN: four paths, in this order —
  1. the decrypted, inflated `[TXT]` section of TTTEXT.ROD, as written by
     `vagcan vcds rod TTTEXT.ROD --dump DIR` (i.e. `DIR/TXT.bin`)
  2. `UStringData.txt` — one text per line, from `UStringData.data.gz` of an
     ODIS project: `u32` char count + UTF-16LE, repeated
  3. `AStringData.txt` — one name per line, from `AStringData.data.gz`:
     `u32` byte count + ASCII, repeated
  4. a `{"<text id>": "<name>"}` catalog of names already read, used as the
     hold-out that measures precision

OUT: `pilot-new.json` next to the working directory, `{"<text id>": "<name>"}`
for every record with a unique candidate. Read `odis-crib.md` §4.1 before
using it: only spans of 12 letters or more survived the hold-out, and this
file is not filtered.

Neither the ODIS data nor the output belongs in the checkout.

--- how it works ---


The TTTEXT cipher is a per-record monoalphabetic substitution acting on three
disjoint classes (research/labels/tttext-codec.md §1):

  a-z and A-Z   permuted by one permutation, case preserved
  0-9 , . _ -   permuted among themselves, independently
  everything else passes through

Such a cipher preserves a signature of the string. Two strings share a
signature exactly when some legal key maps one to the other, so a signature
lookup against a closed candidate list is a complete, sound matcher: no false
negatives, and a unique hit is a solved record.
"""

import json, re, sys, collections

NUM = "0123456789,._-"
NUMSET = set(NUM)


def signature(s):
    """Substitution-invariant signature. Letters and the numeric class are each
    replaced by first-occurrence order within their own class; case is kept
    because the cipher keeps it; everything else is literal."""
    lmap, nmap, out = {}, {}, []
    for ch in s:
        if ch.isascii() and ch.isalpha():
            c = ch.lower()
            if c not in lmap:
                lmap[c] = len(lmap)
            out.append(f"L{lmap[c]}{'U' if ch.isupper() else 'l'}")
        elif ch in NUMSET:
            if ch not in nmap:
                nmap[ch] = len(nmap)
            out.append(f"N{nmap[ch]}")
        else:
            out.append("=" + ch)
    return "\x1f".join(out)


def letters(s):
    return sum(1 for c in s if c.isascii() and c.isalpha())


def text_cuts(payload, min_letters=1):
    """Candidate spans for field 0 of a record.

    Layout is `<text><sep><field>…` with the separator drawn from the numeric
    class (tttext-codec.md §1.3), and the separator is itself enciphered, so
    which glyph it is cannot be read off. But the text is always field 0, so
    every cut point is a position holding a numeric-class glyph — plus the
    whole payload, for records that carry no tail.

    Yields longest first: a longer span is more constrained and therefore the
    stronger claim when it matches.
    """
    seen = set()
    for end in [len(payload)] + [i for i in range(len(payload) - 1, 0, -1) if payload[i] in NUMSET]:
        span = payload[:end]
        if span not in seen and letters(span) >= min_letters:
            seen.add(span)
            yield span


def load_records(path):
    """`NNNNNN,<payload>\r\n`, latin-1, the id plaintext."""
    recs = {}
    # Binary, then decode: text mode would translate CRLF and eat the framing.
    for line in open(path, "rb").read().decode("latin-1").split("\r\n"):
        if len(line) > 7 and line[:6].isdigit() and line[6] == ",":
            recs[line[:6]] = line[7:]
    return recs


def load_odis(u_txt, a_txt):
    cands = set()
    for line in open(u_txt, encoding="latin-1"):
        t = re.sub(r"<[^>]*>", "", line).strip()
        if t:
            cands.add(t)
            # A DESC is often `<key>;<text>`; the text alone is the better crib.
            if ";" in t:
                cands.add(t.split(";", 1)[1].strip())
    for line in open(a_txt, encoding="latin-1"):
        t = line.strip()
        # Short names are identifiers, not prose. Keep only the ones that could
        # plausibly be a displayed text: they still cost nothing to index.
        if 3 <= len(t) <= 120:
            cands.add(t)
    return {c for c in cands if c}


def main():
    txt, u_txt, a_txt, names_json = sys.argv[1:5]

    recs = load_records(txt)
    known = json.load(open(names_json))
    cands = load_odis(u_txt, a_txt)

    index = collections.defaultdict(set)
    for c in cands:
        index[signature(c)].add(c)

    print(f"records         {len(recs)}")
    print(f"ODIS candidates {len(cands)} in {len(index)} signatures")
    print(f"known readings  {len(known)}")

    # --- positive control: the matcher must find a plaintext we hid ourselves ---
    import random

    rng = random.Random(20260807)
    ctrl_ok = ctrl_n = 0
    for plain in list(cands)[:2000]:
        lets = list("abcdefghijklmnopqrstuvwxyz")
        perm = lets[:]
        rng.shuffle(perm)
        lk = dict(zip(lets, perm))
        nk = list(NUM)
        rng.shuffle(nk)
        nkm = dict(zip(NUM, nk))
        ct = "".join(
            (lk[c.lower()].upper() if c.isupper() else lk[c]) if (c.isascii() and c.isalpha()) else (nkm[c] if c in NUMSET else c)
            for c in plain
        )
        ctrl_n += 1
        if plain in index.get(signature(ct), ()):
            ctrl_ok += 1
    print(f"\npositive control: {ctrl_ok}/{ctrl_n} enciphered ODIS texts found again (must be 100%)")

    def best_match(payload):
        """Longest span with any candidate wins; returns (span, hits)."""
        for span in text_cuts(payload, min_letters=2):
            hits = index.get(signature(span))
            if hits:
                return span, hits
        return None, set()

    # --- ground truth: records we already read, matched blind through ODIS ---
    tally = collections.Counter()
    for rid, plain in known.items():
        _, hits = best_match(recs.get(rid, ""))
        if not hits:
            tally["no candidate"] += 1
        elif len(hits) == 1:
            tally["unique, agrees" if next(iter(hits)) == plain else "unique, DISAGREES"] += 1
        else:
            tally["ambiguous, correct among them" if plain in hits else "ambiguous, correct absent"] += 1
    print("\nground truth over the 14,738 records already read:")
    for k, v in tally.most_common():
        print(f"  {v:6d}  {k}")

    # --- the target: records the letter floor rejects ---
    buckets = collections.Counter()
    solved = {}
    ambiguous = 0
    for rid, ct in recs.items():
        n = letters(ct)
        b = "12+" if n >= 12 else ("8-11" if n >= 8 else "<8")
        buckets[b] += 1
        if rid in known:
            continue
        span, hits = best_match(ct)
        if len(hits) == 1:
            (h,) = hits
            solved[rid] = (b, h, span)
        elif len(hits) > 1:
            ambiguous += 1

    print(f"\nunread records by letter count: {dict(buckets)}")
    per = collections.Counter(b for b, _, _ in solved.values())
    print(f"newly matched (unique hit, not already known): {len(solved)}  {dict(per)}")
    print(f"ambiguous (>1 candidate, unusable as-is):      {ambiguous}")

    print("\nsample of new readings:")
    for rid, (b, h, span) in list(solved.items())[:25]:
        print(f"  {rid} [{b:>4}] {span!r} -> {h!r}")

    json.dump({k: v[1] for k, v in solved.items()}, open("pilot-new.json", "w"), ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()
