# Scaling in the corpus — the decode audited, the DID negative re-confirmed

The question this project keeps returning to: can `(read DID, raw form, factor, offset, unit)`
be read out of VW's `.rod` label corpus, so that measurement scalings no longer have to be
proven one at a time by driving the car (`vagcan recording calibrate`, `vagcan vcds analyse`,
`catalogs/vehicles/*.json`)?

This pass does the one thing the earlier writeups explicitly asked the next person to do
before reopening anything: **audit whether the refutations' negatives rested on a correct
decode.** The verdict is split and worth stating up front, because half of it overturns a
premise and half of it survives:

- The decode that `rod-labels.md` §4.0c used to conclude "the read DID is not in `STRUC`" —
  reading the record as a **base-14 bignum** — is the **wrong codec**. It was already
  superseded inside the corpus writeups (the per-table substitution of
  `crates/vag-data-labels/src/glyphs.rs`), but §4.0c's DID tests were never re-run under the
  correct one. So those specific tests were void.
- Re-run under the **correct** decode, the DID search comes back negative at chance level
  anyway. **§4.0c's conclusion survives its broken decode.** The read DID is not a field of
  `STRUC`/`MUX`, and (with `label-linkage.md` §3 and `tttext2.md` §6.2a, both decode-independent
  cardinality arguments) not anywhere per-ECU in the corpus.
- Meanwhile the decode being correct settles the *other* half: the corpus **does** carry the
  fine scalings. `rod-labels.md` §5.3 withdrew the claim that it could not; this pass confirms
  the withdrawal was right, at **100 % table coverage** rather than the 21–38 % the row-order
  attack reached. Every factor this project proved by driving — `0.4`, `0.01`, `0.001`, `1.0`,
  `0.5` — is present in `MUX`, and two full DOPs decode end to end.

**Net for the prize: still NEGATIVE, now sharpened and decode-audited.** A
corpus→`(DID, form, factor, offset, unit)` extractor that reproduces the 20 proven rows is not
achievable, for two independent reasons that both survive the corrected decode: (a) the read
DID is absent from the corpus, and (b) the per-ECU measurement→structure edge is unproven — and
where a measurement name *is* reachable in the global tables, it resolves to the **self-test**
DOP with a different scaling (engine speed `×0.25` vs the proven ADVMB `×1`), so a name-join is
a trap, not a shortcut. What changed is the belief that scaling is *intrinsically* live-only:
it is not. It is in the corpus, machine-readable, and blocked only by the missing join to a
specific car's measurement.

Companion to `rod-labels.md` (§4.0c, §5), `label-linkage.md` (§3), `mux.md`, `tttext2.md`.
Reproduction scripts: `research/labels/scaling-audit/` (`glyphs.py`, `audit.py`).

---

## 0. What was audited, and the oracle

Ground truth is the 20 measurements proven on the reference car and committed under
`catalogs/vehicles/` (12 gearbox `0CW300041G`, 3 engine `8V0906264H`, 8 cluster
`5E0920740D` — read as `(DID, raw form, factor, offset, unit)` each). Ten of the gearbox rows
additionally carry a **name text-id** from the VCDS log's `ENG######` column
(`tttext-codec.md` §2 proved `ENG###### = TTTEXT` id), e.g. `380A` input speed = text-id
`103074`, `F40D` vehicle speed = `99967`. That gives a name-keyed oracle the earlier attempts
did not fully use.

Working data (proprietary, **not committed**): `vendor/vcds-en/UDS_EV/{STRUC,MUX,TTDOP}.rod`
and the gearbox `EV_TCMDQ200021.rod`, inflated with the shipped decoder
(`vagcan vcds rod <file> --dump`, keys from the committed IV cache and the vendor `.ivcache`
sidecars).

---

## 1. The decode is correct — verified on STRUC/MUX/TTDOP, decimals and signs included

`glyphs.rs::TableAlphabet::for_key` generates a table's substitution by seeding the CRT
`rand()` with the table id and Fisher-Yates-shuffling the alphabets. The Rust tests exercise it
on the fault registry (`Codes.dat`); this pass re-ports it (`glyphs.py`, self-test against the
same table-531 vector) and drives it over the three structure tables. **Every check is one a
wrong decode would fail:**

| table | rows | exact field count | bit-offset field ∈ 0..7 | undecodable fields |
|---|---|---|---|---|
| STRUC (11 fields) | 8,853 | **8,853 / 8,853** | **8,853 / 8,853** | 0 |
| MUX (17 fields) | 42,431 | **42,431 / 42,431** | **42,357 / 42,357** | 44 (0.1 %) |
| TTDOP (4 fields) | 127,433 | **127,433 / 127,433** | — | 0 |

TTDOP is `(lower, upper, name-ref, ·)` with `lower == upper` in 121,881 rows (95.6 %) — a
texttable list, no coefficient field, matching `label-linkage.md` §2.2.

**Byte-level hand-verification** (`handcheck`, the case the task asked for — decimal point and
minus sign included). MUX table `19839`, generated alphabet
`5→0 8→1 4→2 ,→3 _→4 6→5 3→6 -→7 9→8 0→9`, separator `7`. Raw record
`…9 · 265 · 5.6 · 8 · , · … · 8543_6` decodes field-for-field to

```
offset f9  '265'   -> -50        ( '2'->'-'  '6'->'5'  '5'->'0' )
num    f10 '5.6'   ->  0.5       ( '.' stays a point )
den    f11 '8'     ->  1
unit   f12 ','     ->  3   (= °C)
name   f16 '8543_6'-> 102645  = "ambient air temperature"
```

i.e. **value = raw × 0.5 − 50 °C** — a standard 8-bit VW temperature scaling. The `.` and the
`-` are carried through as punctuation, not misread as digits. This is the exact failure mode
(`glyphs::decode` once dropping the decimal point) that the cleanup note warns voids a
refutation; it does not happen here.

**Conclusion of the audit:** the current decode is correct. The base-14 model `rod-labels.md`
§4.0c relied on is not — but §5/§5.3 already retired it, and everything below uses the correct
one.

---

## 2. Re-running the DID search under the correct decode — the negative holds

`rod-labels.md` §4.0c searched for the read DID inside `STRUC` as a `u16` and as a **base-14
field**, and found ≤5 hits across 13 crib DIDs (chance). The base-14 reading is the wrong
codec, so that test is void. Re-run over the correctly-decoded field values (`audit.py`
AUDIT 2), the 18 crib DIDs across STRUC's and MUX's every field:

```
STRUC: {14509: 2}                      (field 0 = the TTDOP/MUX reference — a coincidental id)
MUX:   {14395: 2, 14508: 3, 14386: 2}  (field 7 = a TTDOP reference — coincidental ids)
```

A handful of hits, all in **reference** fields (whose values are table ids, not DIDs), none in
a position that could be a stored identifier. This is the same chance-level non-result §4.0c
reported — **the conclusion survives the corrected decode.** The read DID is not a decoded
field of `STRUC` or `MUX`.

That is consistent with the two decode-*independent* negatives it does not rest on: the
per-ECU sections carry only `(text-id, 2-char code)`, the code is a global function of
`(section kind, text-id)` with no per-ECU degree of freedom (`label-linkage.md` §3,
`tttext2.md` §6.2a, 169/169 across the shifted 40 %), and a 2-char code cannot encode a 16-bit
DID. The only unexamined hiding place remains `TTTEXT2.ROD` (`tttext2.md`): uncracked, a
5–11 h sweep, and shaped like a name table.

---

## 3. The positive: the corpus carries the scalings, at 100 % coverage

The decode being correct turns `mux.md`'s partial reading into a complete one. `mux.md` opened
21 % of MUX (the row-order attack needs ~5 rows per table); the generated key opens **all** of
it. Over MUX's 7,384 fully-decoded linear (kind-0) rows (`audit.py` POSITIVE):

```
top factors: 0.1(1065) 0.01(959) 1.0(942) 0.001(833) 0.5(502) 0.2(237)
             0.000788(219) 0.25(179) 0.05(151) 0.000977=1/1024(129) 10(125)
             50(112) 0.4(100) …            offsets include -40, -50, -327.68
```

Every proven factor is present: `0.4` (accelerator, ×100 rows), `0.01` (×959), `0.001`
(boost, ×833), `1.0` (×942), `0.5` (×502). Two DOPs decode end to end and are physically exact:

- **`102645` "ambient air temperature"** → `raw × 0.5 − 50 °C` (§1); a sibling record
  `420177` "ambient air temperature value" → `raw × 1 − 40 °C`, which **matches the proven
  temperature family** (`rod-labels.md` §4.3a, `raw − 40`).
- **`103124` "Idle Speed Commanded Value"** → MUX table `16606`: a 16-bit value at `×1/4 /min`
  (unit 21) with a multiplexer sending the raw `2550` to a text (TTDOP `5041`, an
  invalid-value marker) — exactly the ODX shape of an rpm measurement.

So "the corpus cannot express the scaling" (an early NO-GO) and "scaling is only ever live"
(the working assumption) are both **false**. The scaling is there and readable.

---

## 4. Why it still does not reach the 20 proven rows

The scalings are in the corpus; the **join from a car-readable handle to them is not.** Two
walls, both re-confirmed here with the oracle:

**4.1 The ADVMB measurements are not name-reachable in the global tables.** Of the 11 gearbox
measurement text-ids from the `ENG` log, **9 are absent from both `STRUC` f9 and `MUX` f16**
(`audit.py` NEGATIVE). The gearbox has no structure section of its own; its DOPs must be
reached from its `MWB` via the code, and the global tables simply do not carry them under the
measurement's name.

**4.2 Where a name *is* present, it is the wrong DOP.** The two that resolve (`98363`
accelerator, `103124` idle) are **self-test** entries — MUX is dominated by `Test_Program_*`
names. Searching MUX for "engine speed" returns `Test_Program_Engine speed` at **`×0.25 /min`**,
where the proven ADVMB engine speed (`206E`) is **`×1`**. A name-join to MUX would therefore
ship a factor four times wrong. This is exactly the class of confident-but-wrong result the
project guards against, and it is why the positive of §3 is not quietly an extractor.

**4.3 The per-ECU `code → structure id` edge is refuted again.** Using the gearbox `MWB` rows
whose text-id happens to appear in MUX (72 of 1,020), the 2-char code does **not** determine the
id: 64 of 69 codes map to more than one id, ~60 different codes for the `PIM_measured_Phase_*`
family all "resolve" to the same five MUX tables, and code `4-` maps to two disjoint id sets.
The apparent resolution is by **name** (a MUX multiplexer names many signals), not by code. This
is an independent confirmation of `rod-labels.md` §3.1 and `label-linkage.md` §3's refutations of
`code → STRUC/MUX-id`, now from the proven end.

So the single missing edge is unchanged from `mux.md`'s closing sentence — *the one hop from a
per-ECU `.rod` row to a structure id* — and the DID is absent regardless. Both survive the
decode being fixed.

---

## 5. Verdict, and answer to "did the old decode hold up"

- **Old decode held up? No — and yes.** The base-14 codec of `rod-labels.md` §4.0c is wrong
  and its DID tests were void. But the negative it reported is reproduced under the correct
  codec, so its **conclusion** stands. The `rod-labels.md` §5.3 withdrawal (corpus *does* carry
  fine scalings) is confirmed correct. `label-linkage.md` §3 and `tttext2.md` §6.2a never
  depended on the numeric decode and are unaffected.
- **Extractor delivered? No.** The read DID is not in the corpus (re-audited), and the
  measurement→scaling join is either absent (§4.1) or points at the wrong DOP (§4.2); the
  `code → id` edge is refuted from the proven end (§4.3). A corpus→`(DID, …)` extractor
  reproducing the 20 rows cannot be built on what is decodable today.
- **What is now different.** Scaling is no longer "live-only in principle." The corpus carries
  it, correctly decodable at 100 % coverage, and two DOPs were read out end to end (one
  matching a proven scaling family). Live calibration remains the only *proven* route to a
  specific car's `(DID → factor)` — because the corpus does not say which DID a measurement is
  read at, not because it does not know the factor.

**Do not re-run:** the base-14 DID search (wrong codec, §2); a MUX/STRUC name-join as a source
of ADVMB scaling (§4.2, the self-test trap); `code → id` in any form (§4.3). **The only open
door** is `TTTEXT2.ROD` (`tttext2.md`) for a global `measurement → DID` registry, and the
runtime-mask cost it carries.

---

## 6. Reproduction

```
BIN=target/release/vagcan ; V=vendor/vcds-en/UDS_EV
cargo build --release -p vagcan
$BIN vcds rod $V/STRUC.rod --dump /tmp/rodwork/struc     # STRUC.bin  293,560 B
$BIN vcds rod $V/TTDOP.rod --dump /tmp/rodwork/ttdop     # DOP.bin  2,722,454 B
$BIN vcds rod $V/MUX.rod   --cache catalogs/rod-iv-cache.json --dump /tmp/rodwork/mux
$BIN vcds rod $V/EV_TCMDQ200021.rod --dump /tmp/rodwork/gbx   # MWB.bin  11,220 B

python3 research/labels/scaling-audit/glyphs.py   # selftest vs table 531
python3 research/labels/scaling-audit/audit.py    # all of §1–§4
```

`glyphs.py` is a faithful port of `crates/vag-data-labels/src/glyphs.rs`; the audit reads only the
inflated blobs plus `catalogs/names-uds.json`. No car-specific number is embedded — the crib
lives in `catalogs/vehicles/*.json` and the `ENG` pairings in `tttext-codec.md` §2.
