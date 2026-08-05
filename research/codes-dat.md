# `Codes.dat` — the fault-text store, opened

What VCDS ships beside its label corpus, what is inside it, and — the part that matters
for this project — what it can and cannot name on the reference car.

Target: `Codes.dat`, 2 121 476 bytes, from a freshly extracted **VCDS 26.3** installation.
Cross-checked against `research/VCDS-RUS/Code-RUS.dat` (the Russian translation of the same
file) and against `research/VCDS-25.12.0/`. Nothing third-party is committed; the decoder is
`crates/vag-data/src/codes.rs`.

Read `research/whole-car-survey.md` §3 first — this file finishes what it started and
**supersedes two of its statements**, listed in §6.

---

## 0. Verdict up front

| question | answer | confidence |
|---|---|---|
| Record structure? | **Solved** — §1 | very high (34 716/34 716 frame cleanly) |
| Encrypted, compressed, or opaque? | **Encrypted only** — TEA-CBC under the `.rod` key — §2 | very high |
| Is the per-record block-0 IV recoverable? | **Yes, and it is now derived, not guessed** — §2.2 | very high (read off the binary, confirmed on 62 303 records in two languages) |
| Does it contain fault text? | **Yes, in full** — §3 | very high |
| Did a known fault code produce its known text? | **Yes** — `B1168 F2` → "Steering Angle Sensor: Not Initialized" — §3.2 | very high |
| Can `vagcan faults` name what this car reports? | **No — one hop still missing** — §4 | high, and it is a clean negative |
| Does it finish the `RD.rod` chain? | **The naming half, yes** — §5 | high (47/48 against an 8.2 % baseline) |
| Is the missing hop still missing? | **No — closed, and the command ships** — §7 | see `research/fault-naming-hop.md` §11–§12 |

The headline is §3 together with §4. The file is completely open, including the eight
characters per record that the previous pass wrote off as lost — but its keys are **ISO/SAE
DTCs**, and a VAG control unit answers `0x19` with a **VW-internal fault number**. Naming the
faults this tool already reads still needs that one conversion, and this file does not carry
it. What it does carry is everything on the far side of it: once a fault number has been
turned into an ISO DTC, the name is a dictionary lookup with no TTTEXT, no `names-uds.json`
and no digit substitution anywhere in the path.

---

## 1. The container

```text
record := <8 ASCII digits: key> ' ' <u8 cipher_len> <u8 text_len> <cipher> "\r\n"
```

Variable length, no index, no header, no trailer. Records appear in strictly ascending key
order — 34 716 of them, keys `00000000` … `33554431`, with no duplicates.

`cipher_len` is `text_len` rounded up to a multiple of 8 (34 716/34 716), which is the first
sign of an 8-byte block cipher. Longest record: `text_len` 88.

**The trailing `\r\n` is decoration, not framing.** Ciphertext is binary and does contain
`0d 0a`; a reader that splits on newlines loses records and silently mis-joins others. The
shipped decoder walks by declared length and has a test for exactly this.

VCDS's own loader agrees, and is worth quoting because it settles the framing without
inference — `fcn.140052ed8` in `VCDS-ARM.exe` 26.3: it reads the file line-wise, checks
`isdigit` on the first eight bytes one at a time, `sscanf`s them with `"%08d"` into an `int`
array at `0x140530850`, then copies `cipher_len + 11` bytes from offset 9 into a fixed-stride
record table at `0x14020d4c0` (stride `0x5e` = 94, cap `0x88b8` = 35 000 records). Nothing is
decrypted at load time.

---

## 2. The payload

### 2.1 Encrypted, not compressed, and under a key this project already had

Ross-Tech reuses its machinery, and the first thing to try was the machinery. `KEY_ROD`
(`crates/vag-data/src/rod.rs`), TEA 32 rounds, CBC, little-endian words — the `.rod` cipher —
decrypts every record. There is no compression layer: `text_len` bytes of Latin-1 (or CP1251
in the Russian file) come straight out.

Entropy never had to be measured to answer "which of the three": with a wrong IV, CBC still
yields correct plaintext from block 1 onward, so the very first attempt returned
`b'\xe2"\x91\x1c\x99=\r\xed' + "sion Control Unit"` for key 2 — visibly `Transmission
Control Unit` with its first eight characters destroyed. That is the signature of a block
cipher in CBC with an unknown IV, and nothing else.

### 2.2 The block-0 IV — derived, not brute-forced

`whole-car-survey.md` §3 recorded this IV as "per-record and unsolved, so the first 8
characters of each text are lost". It is solved.

Two statistical models were tried first and **both are refuted**, recorded so nobody repeats
them:

* *IV depends only on `(position, seed[1], digit at that position)`* — the shape the `.rod`
  derivation has. Grouping 34 716 records that way and solving each group against a
  byte-frequency model gives noise: with the known-true IV for the `Steering` records
  substituted in, the character distribution of a 900-sample group is flat
  (`C`×9, `â`×9, `\t`×7, …). Refuted.
* *cross-record CBC chaining* (IV = previous record's last/first cipher block, previous
  plaintext block, the key as ASCII/LE64/BE64, the record index) — eight variants, none
  produced text.

What did work was reading it. The consumer is `fcn.1400e1400` in `VCDS-ARM.exe` 26.3,
`0x1400e1908`–`0x1400e19dc`. In the project's existing vocabulary:

```text
seed  = sprintf("%08d", key)          # the key's own decimal spelling, all 8 bytes
m     = seed[5]                       # NOT seed[1], which is what .rod uses
s[i]  = seed[i] + KS[(m*(i+2)) & 0xff] + C
IV[i] = s[i] * MT[OFF_ROD[7 - i]]     # OFF_ROD reversed
```

all arithmetic `u8` wrapping, `MT`/`KS`/`OFF_ROD` exactly the tables already in
`crates/vag-data/src/`. Three things were new and each is worth naming, because each one is
why the statistical attacks failed:

* the **seed is the key string**, so the IV moves with all eight digits at once;
* the `KS` index is driven by **seed byte 5**. That is the whole reason the
  `(position, seed[1], digit)` model collapsed — records sharing `seed[1]` do not share an IV;
* the multiplier offsets are `OFF_ROD` **in reverse**, which is why `MT[OFF_ROD[0]] = 0x7d`
  turned up as the *last* position's step. That step, 0x7d per increment of the last digit,
  was the observable that first said the derivation was in this family at all.

`C` is a **file-wide byte VCDS keeps in a global** (`0x140552ba4`), filled at load time and
therefore not in the file. It is **0** for the English `Codes.dat` and **208** for
`Code-RUS.dat`. The decoder recovers it the way `clb.rs` recovers `w7` — but with a better
score than printability, which does not separate the candidates: blocks 1.. are correct
whatever `C` is, so their byte distribution is a free, language-agnostic model of the file's
own text, and the winning candidate is the one whose first blocks look like the rest of the
same file. On `Code-RUS.dat`, `C = 208` and `C = 80` both give **fully printable** first
blocks and only one gives Cyrillic; the frequency model separates them by 522 000 nats.

### 2.3 It decrypts completely

| file | records | text bytes | control bytes in the output | `C` |
|---|---|---|---|---|
| `Codes.dat` (EN 26.3) | 34 716 | 1 547 836 | **0** | 0 |
| `Code-RUS.dat` (RU) | 27 587 | 1 432 611 | **0** | 208 |

Zero control bytes across 1.5 MB of recovered text is the check that would have caught a
partly-wrong IV: a single wrong first block shows up as ~8 control characters, and the
pre-crack decrypt had 30 599 of them in the Russian file.

### 2.4 The bytes are a code page, and it is not the obvious one

The table above counts *bytes*. Turning them into characters is a separate step, and the
first version of the decoder did it by mapping each byte straight to a `char` — ISO
8859-1 — which is wrong for both files:

| file | page | what 8859-1 did to it |
|---|---|---|
| `Codes.dat` (EN) | Windows-1252 | `0x96` is an en dash; 8859-1 makes it U+0096, a C1 control. 191 occurrences, in 216 records |
| `Code-RUS.dat` (RU) | Windows-1251 | 27 380 of 27 587 records became mojibake — `Äàò÷èê` for `Датчик` |

Nothing in a record says which page it is, so `CodesDb::parse` takes the English one and
`parse_in` asks. Re-run against both files with the pages named: **0 control and 0
undefined characters** in either.

§2.3 is unaffected — it counts bytes below 0x20 in the plaintext, and 0x96 is not one.
The two checks catch different things: §2.3 proves the decryption is exact, and this one
proves the text is then read in the page it was written in. Only the second would have
noticed that a correct decrypt was being displayed as `Äàò÷èê`.

---

## 3. What is in it

### 3.1 Two key bands, and the boundary matters more than the contents

| band | keys | count | what they are |
|---|---|---|---|
| low | 0 … 65 535 (sparse) | 4 825 | legacy 5-digit VAG fault codes, KWP era |
| high | 90 000 … 0x1FFFFFF | 29 891 | the **24-bit ISO/SAE DTC**, big-endian |
| — | 33 554 431 (`0x1FFFFFF`) | 1 | a sentinel: "English-Language Clarifications to these texts Copyright © 2025 Ross-Tech LLC" |

Nothing at all sits between 65 536 and 89 999.

The high band reads as `system:2 | code:14 | failure_type:8`, i.e. exactly the three bytes a
UDS unit puts in a `0x19` response, read as one big-endian number. Evidence that this is the
right reading and not a story:

* **2 306 distinct 16-bit codes** carry 29 891 records — an average of 13 failure types per
  code, which is what a `(DTC, FTB)` table looks like and not what a flat id space looks like;
* the failure-type byte clusters exactly where ISO 14229-1 Annex D puts it: `0xF0` (2 078),
  `0xF1` (1 012), `0xF2` (670), `0xF3` (492), descending;
* the system letter splits P 22 045 / B 5 917 / C 1 830 / U 99, and the texts match the
  letter — the 99 `U` codes are bus and communication faults;
* consecutive keys sharing a 16-bit code share a description stem and differ in the tail:
  `0x9168F0..F3` are "Steering Angle Sensor: Rate of Change to High / Synchronization Failed /
  Not Initialized / Offset too high".

### 3.2 The reference car's own faults, named

`research/eps-j500-report-ru.md` records the three codes the steering assist has stored since
the sweep, with their failure-type bytes. All three resolve:

| code | key | `Codes.dat` (EN) | `Code-RUS.dat` |
|---|---|---|---|
| `B1168` FTB `F2` | 9 529 586 | **Steering Angle Sensor: Not Initialized** | Датчик угла поворота рулевого колеса: отсутствует инициализация |
| `B200F` FTB `F0` | 10 489 840 | **Internal Fault: -** | Внутренняя неисправность |
| `B2000` FTB `F0` | 10 486 000 | **Control Module: Defective** | — |

The first row is the strongest single piece of evidence in this file, and it is worth being
precise about why. "Steering" is **exactly eight characters** — the eight the unknown IV had
been destroying. Recovering them is what turned `… Angle Sensor: Not Initialized` into the
named fault, and the IV derivation was confirmed against that word before it was implemented.

`B2000` is stored on the car with failure type `00`, and key 10 485 760 (`0xA00000`) is
**absent** — only `F0..F5` exist for that code. The `F0` text is quoted above because it is
what VCDS would show for the same code with a failure type it has; the honest statement is
that `Codes.dat` has no entry for `B2000 00`.

---

## 4. The negative, and it is the important half

**A VW-internal fault number is not a key in either band.** Every number the reference car
actually reports, and every number VCDS printed in its own scan of it, is absent:

| number | where it came from | in `Codes.dat`? |
|---|---|---|
| 229 504, 7 680, 19 716, 20 228 | the EPS unit's `0x19` response, `research/dumps/eps-fault-229504-before-clear.txt` | no |
| 16 136, 291 104 | the same car, `research/eps-j500-report-ru.md` and the VCDS scan | no |
| 17 178, 15 187, 16 275, 12 289, 26 885, 197 225, 589 825 | `research/VCDS-RUS/Scans/…20260731…` — VCDS named every one of these | no |

So the file is not a fault-number dictionary, and `vagcan faults` cannot be wired to it as it
stands.

**And a naive lookup is worse than no lookup.** Fault 297 is the trap
`whole-car-survey.md` §3 already flagged, now with its mechanism: the brake unit reports DTC
`00 01 29`, which as a number is 297, which falls in the **legacy** band, where `Codes.dat`
answers "Gearbox Speed Sensor (G38)" — a confident, plausible, wrong name for what the car
means. The two spaces overlap numerically below 65 536 and the file does not distinguish
them. `CodesDb::iso_dtc` therefore refuses anything below 90 000 rather than answer from the
legacy band, and there is a test pinning that refusal to this exact case.

---

## 5. What it does finish: the far half of the `RD.rod` chain

`whole-car-survey.md` §3 established that `UDS_EV/RD.rod`'s `[DTC]` section is a global
registry keyed by the fault number (946/946), that its per-table digit substitution is broken
by the row ordering (`crates/vag-data/src/glyphs.rs`), and that the decoded values "read as
24-bit fault codes" — but that they resolved against `names-uds.json` *below* chance, so
nothing named them.

They resolve against `Codes.dat`. Decoding `RD.rod [DTC]` (6 577 695 bytes, 236 755 rows,
110 767 tables; `IV[3..8] = 5c b0 48 d4 3f`) and running the ordering attack:

| row field | distinct values | in the ISO band | present in `Codes.dat` | digit-shuffled baseline |
|---|---|---|---|---|
| **field 0** | 48 | 48 | **47 (97.9 %)** | 8.2 % |
| field 1 | 10 | 0 (all small) | — | — |
| field 2 | 61 | 61 | **0 (0.0 %)** | 3.0 % |

The high band's own density is 0.089 % of its numeric range, so 8.2 % is already a generous
baseline (digit-shuffling preserves magnitude, and the file is clumpy). 47 of 48 against it
is not a coincidence, and the contrast with field 2 — 61 values, all in range, none present —
is what makes it an identification rather than a hit rate: **field 0 is an ISO DTC and field
2 is a different id space**, consistent with §3's conjecture that the latter is `TTTEXT2.ROD`.

The names are self-evidently right, which is the last check: `120650` → "Steering Torque
Sensor", `140960` → "Internal Control Module Memory Check Sum Error", `153537` → "Databus:
Missing Message", `102708` → "DC/DC Converter: Inadequate Performance".

**Caveat, stated plainly.** This ran on **4 tables of the 8 193 with ≥5 rows**, because the
separator rule used here (all rows of a table end in the same glyph, that glyph is the
separator) is stricter than the one behind the 680 tables §3 reports. The sample is small.
What it is not is ambiguous: 48 values is already far past the point where 97.9 % against
8.2 % could be noise, and every decoded name is a real fault description.

**What this does *not* give** — and §3 said so first: `RD.rod` rows are cross-references, not
self-identification. The reference car's own tables do not name themselves. Table `007680`
(16 rows) does not decode to `0xA00FF0` under any consistent alphabet; its ordering
constraints pin only 7 glyphs, so it does not solve at all. Table `229504` has 4 rows and
`016136` has 1. So the fault → its own ISO code hop is still open, and it is now the *only*
open hop.

---

## 6. Corrections to `whole-car-survey.md` §3

Two statements there are superseded and should be read against this file:

1. *"The block-0 IV is per-record and unsolved, so the first 8 characters of each text are
   lost; the rest is exact."* — solved (§2.2). Both language files decrypt in full.
2. *"`Codes.dat` lacks 5386, 6922, 291104, 14751, 15187 and 25548 entirely."* — true, but the
   framing invites the wrong conclusion. It is not that those particular numbers are missing;
   it is that **no** VW fault number is a key, because the keys are ISO DTCs (§4). The
   observation was right and the inference to draw from it is stronger than the one recorded.

Still true and untouched: the key is not the VW fault number, and looking one up returns a
plausible wrong answer.

---

## 7. What it took to wire this into `vagcan faults` — done

**Step 1 below is closed and the command ships.** `research/fault-naming-hop.md` found the
hop and it is not the one this section guessed at: the join is `UDS_EV/RD.rod`'s `[DTC]`
registry, keyed by the raw VW number in decimal, with the row chosen by the unit's own
`.rod` and the row's fields read through a substitution the table key generates
(`srand(key)`, that file's §11). Candidate (c) — read it off the ARM64 binary — was the
one that paid, again, and in the same function §2.2 read the record IV out of.

Two things this section got right and one it got wrong, worth keeping:

* right: reading the binary is cheaper than attacking it, twice now;
* right: **step 3.** `iso_dtc`'s refusal below 90 000 was never on the shipped path in the
  end — the key comes out of a registry row, not out of a fault number — but the *rule*
  is what `vag_data::codes::sae_code` now enforces at the same boundary, and what
  `UnitCatalogue::is_consistent_with_registry` enforces one hop earlier;
* wrong: **step 2 has not been needed.** `Codes.dat` parses in well under a second and
  `vagcan faults` opens it once per run, so the SQLite slot was never built. The cost that
  did need caching was elsewhere — the `[DTC]` section keys, in `catalogs/rod-iv-cache.json`.

The rest of this section is left as written, for the record.

---

The decoder is shipped: `vag_data::CodesDb` — `parse`, `get`, `iso_dtc`, `file_constant`.
`crates/vagcan/src/faults.rs::format_code` already computes the exact key shape
(`u32::from_be_bytes([0, code[0], code[1], code[2]])`), so on the tool side the join is one
call. Nothing was wired, deliberately, because on this car it would name nothing.

In order:

1. **The missing hop: VW fault number → ISO DTC.** Everything else is done. Candidates, in
   the order they look worth trying: (a) a `Codes.dat`-sibling file in the install that has
   not been looked at — the loader at `fcn.140052ed8` is one of several `.dat` consumers;
   (b) a *different* `RD.rod` section, since only `[DTC]` has been decoded and the pairing
   `<VW number, ISO code>` is exactly what a registry would hold once per fault rather than
   as cross-references; (c) the ARM64 binary again — VCDS prints both numbers side by side,
   so the conversion is in there, and §2.2 shows that reading it is cheaper than attacking it.
2. **A corpus slot for it.** `Codes.dat` is one file per language, ~35 k rows, so it belongs
   in the SQLite cache beside the label corpus rather than being parsed per invocation
   (recovering `C` costs a pass over the file).
3. **The band rule enforced at the boundary, not at the call site.** `iso_dtc` already
   refuses below 90 000. Whatever supplies the ISO DTC must go through it, or fault 297
   comes back as a gearbox speed sensor.

Until step 1, the honest behaviour is the current one: print the code, do not invent a name.
That rule is why this file is a result and not a feature.

---

## 8. Reproducing

Nothing here needs the car. `vag_data::CodesDb::parse` on any `Codes.dat` reproduces §1–§3;
the IV vector for key 9 529 586 (`47 02 c8 cd 6c 50 dc d3`) is pinned by a unit test, as is
the constant recovery, the CRLF-in-ciphertext framing and the legacy-band refusal. §5 was
done with throwaway scripts against `~/vcds-en/UDS_EV/RD.rod`; the ordering attack is
`crates/vag-data/src/glyphs.rs` and the `[DTC]` IV tail is recorded above.
