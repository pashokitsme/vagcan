# Control units other than engine and gearbox

What two passive CAN captures say about the rest of the car (Škoda Octavia III, 1.8 TFSI,
DQ200). Everything here comes from the bus itself; the only external cross-check is the
VCDS cluster log `research/logs/LOG-17-IDE00025_&3.CSV`.

Sources:

| file | length | anchor (local) | frames |
|---|---|---|---|
| `research/logs/1.jsonl` | 956 s | 2026-08-01 01:47:48.537419 | 10 928 |
| `research/dumps/session-2026-08-01.jsonl` | 309 s | 2026-08-01 01:17:21.293818 | 3 164 |

Method: ISO-TP reassembly per CAN id (same state machine as
`crates/vag-uds-can/src/sniff.rs`), then `0x22` requests paired with the next `0x62`/`0x7F`
on the answering id, using `response_id_for` from `crates/vag-cli/src/analyse.rs`
(`0x7E0..0x7E7 → +8`, `0x700..0x7BF → +0x6A`). Every request in both captures asked for a
**single** identifier, so record splitting is unambiguous and no response had to be
discarded. 50 partial assemblies were dropped in `1.jsonl` (0.6 % of PDUs) and none of
them affected a non-powertrain unit.

Both captures are VCDS sessions. Nothing in either file is the car talking to itself —
apart from one periodic broadcast (below), the OBD-port bus carries **only** diagnostic
traffic. That is a constraint on everything that follows: nothing can be learned here
that VCDS did not ask for.

---

## 1. Which units are on the bus

Nine units answered something. `0x700` is the address VCDS sends its broadcast
TesterPresent to and never answers.

| request | response | answered | identified as |
|---|---|---|---|
| `0x70C` | `0x776` | full ident + coding | **steering column module** — `5Q0 953 521 KM`, `Lenks.Modul` |
| `0x70E` | `0x778` | full ident + coding + 5 live groups + 2 sub-systems | **body control module** — `5Q0 937 084 CF`, `BCM MQBAB M+` |
| `0x710` | `0x77A` | ident + the routing/installation lists | **gateway** — `3Q0 907 530 B` |
| `0x714` | `0x77E` | full ident + coding + 4 live measurements | **instrument cluster** — `5E0 920 740 D`, `KOMBI` |
| `0x715` | `0x77F` | session control only | *unknown* |
| `0x74A` | `0x7B4` | `2A26`/`2A28`/`2A2E` only | *unknown* |
| `0x74B` | `0x7B5` | `2A26`/`2A28`/`2A2E` only | *unknown* |
| `0x773` | `0x7DD` | session control only | *unknown* |
| `0x7E0`/`0x7E1` | `0x7E8`/`0x7E9` | — | engine / gearbox, out of scope |

`0x773 → 0x7DD` is a **new pair** not previously listed in `analyse.rs`'s doc comment; it
falls out of the existing `+0x6A` rule, so no code change is implied, but the doc comment
could name it. Same for `0x715 → 0x77F` and `0x74A → 0x7B4`.

### The one non-diagnostic message on the bus

A 29-bit frame `0x17F00010`, payload `20 10 00 00 00 00 00 80`, 1913 times in `1.jsonl`
and 610 times in the session capture — exactly 2 Hz in both, byte-identical every time.
The id's low byte `0x10` and the payload's second byte `0x10` both equal the gateway's
address in the numbering established in §3. **Consistent with** a network-management
heartbeat from the gateway; nothing in the capture proves it, and a constant payload
proves nothing about its fields.

---

## 2. Identification identifiers, verbatim

These are byte-exact from the bus. No inference is involved in the *values*; the
inference is only in what the part-number groups mean.

### `0x776` — steering column module (request `0x70C`)

| DID | ASCII / bytes |
|---|---|
| `F187` part number | `5Q0953521KM` |
| `F189` software version | `0245` |
| `F191` hardware number | `5Q0953569B ` |
| `F197` component | `Lenks.Modul  ` |
| `F19E` ODX label file | `EV_SMLSVALEOMQBLRH` |
| `F1A0` | `V03935258VQ` |
| `F1A1` | `0001` |
| `F1A2` | `001007` |
| `F1A3` | `100` |
| `F1A5` | `00 00 28 3F 1E D6` |
| `F1DF` | `40` |
| `0600` coding | `10 14` (2 bytes) |

### `0x778` — body control module (request `0x70E`)

| DID | ASCII / bytes |
|---|---|
| `F187` | `5Q0937084CF` |
| `F189` | `0236` |
| `F191` | `5Q0937084CF` |
| `F197` | `BCM MQBAB M+ ` |
| `F19E` | `EV_BCMMQB` |
| `F1A0` | `V03935259NW` |
| `F1A1` | `0001` |
| `F1A2` | `017001` |
| `F1A3` | `H34` |
| `F1A5` | `00 00 2E 2D 1E D6` |
| `F1DF` | `40` |
| `0600` coding | 30 bytes, **all zero** |
| `0608` | `0001 0002 FFFF ×17` |

### `0x77A` — gateway (request `0x710`)

`F187` = `F191` = `3Q0907530B `. VCDS never asked this unit for `F197` or `F19E` in
either capture, so the component string and the ODX label file name for the gateway are
**not known**. `539B` was requested five times and answered NRC `0x31` every time.

### `0x77E` — instrument cluster (request `0x714`)

| DID | ASCII / bytes |
|---|---|
| `F187` | `5E0920740D ` |
| `F189` | `8311` |
| `F191` | `5E0920740D ` |
| `F197` | `KOMBI        ` |
| `F19E` | `EV_DashBoardVDDMQBAB` |
| `F1A0` | `-----------` |
| `F1A1` | `----` |
| `F1A2` | `009051` |
| `F1A3` | `202` |
| `F1A5` | `00 00 28 3F 1E D6` |
| `F1DF` | `40` |
| `0600` coding | `03 A4 01 08 20 80 00 08 10 08 3A 00 10 01 00 08 00 00 00 00` |

**Cross-check against the VCDS log.** The CSV header line reads
`5E0 920 740 D,ADVMB,KOMBI         202 8311`. `5E0 920 740 D` is `F187`, `KOMBI` is
`F197`, `202` is `F1A3` and `8311` is `F189` — four independent fields matching, which
confirms both the addressing (`0x714 → 0x77E`) and the meaning of the four DIDs.

### `0x77F`, `0x7B4`, `0x7B5`, `0x7DD` — unidentified

None of these was ever asked for `F187`/`F197`/`F19E`. All four answered
`10 03` (extended session) with `50 03 00 32 01 F4`, so they are alive.

* `0x7DD` (`0x773`) and `0x77F` (`0x715`): every DID VCDS tried (`2A2A`, `2A2E`, `F1B7`)
  returned NRC `0x31` (requestOutOfRange). **Nothing at all is known about what they are.**
* `0x7B4` (`0x74A`) and `0x7B5` (`0x74B`): answered `2A26` = `01`, `2A28` = `00`,
  `2A2E` = `4A 00` / `4B 00` respectively. `04A3` and `2A2A` → NRC `0x31`.

### Notes on the shared fields

* `F1A5` takes exactly two values across the whole car: `0000283F1ED6` on the steering
  column module, cluster and gearbox, `00002E2D1ED6` on the BCM and engine. The trailing
  `1ED6` is common to all five. Two groups, two values — **consistent with** a
  workshop/coding stamp written at two different service events. Not established.
* `F1DF` is `40` on every unit that answered it.
* `F1A0`/`F1A1` are `0x31` (not supported) on engine and gearbox but supported on the
  steering column module and BCM; the cluster answers with a filler string of `-`
  characters, i.e. supported but unprogrammed.

---

## 3. The gateway's lists — the highest-value structural finding

In the session capture the gateway answered three list DIDs:

```
2204A3 → 62 04A3 01 54 3C 00 00 00 00 00 40 0C 00 00 80 00 C8 00  00×16
222A26 → 62 2A26 01 54 3C 00 00 00 00 00 40 0C 00 00 80 00 C8 00  00×16   (identical)
222A28 → 62 2A28 01 50 08 00 00 00 00 00 00 00 00 00 40 00 00 00  00×16
222A2E → 62 2A2E 10 01 02 03 … 0F 00 11 12 13 … FF                (256 bytes)
```

**`2A2E` is a list of diagnostic addresses, own address first.** The gateway's list is the
identity permutation `0x00..0xFF` with `0x00` and `0x10` swapped — i.e. `0x10` moved to
the front. `0x10` is the low byte of the gateway's own request id `0x710`. The units at
`0x74A` and `0x74B` return `4A 00` and `4B 00` — again their own low byte first. Three
units, three self-consistent answers. **Confidence: high** for "element 0 is the unit's
own address"; the meaning of the trailing `00` on the two-byte answers is not established
(terminator vs. a listed address `0x00`).

**`2A26`/`04A3` is a 256-bit installed-unit bitmap, LSB-first, indexed by request-id low
byte.** Decoding byte *i* bit *b* → address `8i + b`:

```
00  0A  0C  0E  12  13  14  15  46  4A  4B  67  73  76  77      (15 bits set)
```

as request CAN ids:

```
700 70A 70C 70E 712 713 714 715 746 74A 74B 767 773 776 777
```

Every unit observed answering on the VW block — `0x70C`, `0x70E`, `0x714`, `0x715`,
`0x74A`, `0x74B`, `0x773` — is in that set, and `0x700` is the address VCDS broadcasts
TesterPresent to. That is 8 of 15 bits corroborated by independent traffic and **zero
false negatives**. The MSB-first reading gives `07 09 0B 0D 12 13 14 15 41 4C 4D 60 70 71
74`, which contains only two of the seven observed units, so the bit order is settled.

Absent from the list: `0x10` (the gateway itself) and `0xE0`/`0xE1` (engine and gearbox,
which live on the ISO `0x7Ex` block). **Consistent with** the list being "units I route to
on the VW `+0x6A` block", not "everything installed in the car".

*Open problem.* `0x776` and `0x777` appear as *request* ids in the list, but `0x776` is
already the *response* id of `0x70C`, and `0x776 + 0x6A = 0x7E0` collides with the
engine's request id. Either the `+0x6A` rule does not extend to the top of the block, or
the bit index is not the request-id low byte for those two. **Do not extend
`response_id_for` past `0x773` on the strength of this.**

**`2A28` is the same bitmap format, and a strict subset:** `{00, 0C, 0E, 13, 76}` — five
of the fifteen. Every set bit of `2A28` is also set in `2A26`, in all four bytes where
they differ. What the subset *means* is **not determined**. Candidates that fit equally
well: units with stored fault codes, units flagged "not communicating", units that are
themselves sub-bus masters. Two candidates that fit equally prove neither.

Practical value: **one read of `0x710`/`2A26` enumerates the car's control units.** That
replaces blind scanning of `0x700..0x7BF`.

---

## 4. Live measurements

### 4.1 Instrument cluster `0x77E` — four measurements, three of them proven

VCDS polled exactly four identifiers on the cluster during the window the CSV covers, and
the CSV carries exactly four measurements. The capture's anchor and the log's header time
give the offset **arithmetically** — nothing was searched for:

* log block 1 header `02:01:49` → capture t = 841.463 s
* log block 2 header `02:02:02` → capture t = 853.463 s

| DID | n | bytes | distinct | values |
|---|---|---|---|---|
| `2203` | 139 | 3 | 1 | `03 3F 18` |
| `22D0` | 112 | 1 | 1 | `B8` |
| `22D2` | 109 | 2 | 4 | `0000`, `0002`, `0003`, `0005` |
| `2B3C` | 85 | 1 | 2 | `00`, `01` |

**`2203` = odometer, u24 big-endian, km, factor 1.** `0x033F18` = 212 760, exactly the
`IDE00301` value in the log. A three-byte value landing on a six-digit decimal with no
scaling is not a coincidence available to any other candidate. *Confidence: high*, with
the caveat that the value is constant, so only the encoding at this one point is shown —
a drive would confirm it counts up.

**`22D2` = road speed, u16 big-endian, km/h, factor 1.** Proven by timing, not by fitting:

| capture t | `22D2` | t − 853.463 | log `IDE00075` |
|---|---|---|---|
| 855.81 | `0002` | 2.35 | 2 at t = 2.41 |
| 856.61 | `0003` | 3.15 | 3 at t = 3.21 |
| 857.40 | `0005` | 3.94 | 5 at t = 4.00 |
| 862.22 | `0002` | 8.76 | 2 at t = 8.82 |
| 863.02 | `0000` | 9.56 | 0 at t = 9.61 |

Five transitions, four distinct raw levels, a constant 0.06 s lag between the bus
transition and the value appearing in VCDS's log. During log block 1 (t = 841.5…851.2)
the raw was `0000` throughout and the log reads 0 throughout. *Confidence: high.*

**`2B3C` = parking brake status, 1 byte, `00` = released, `01` = applied.** Same method:
the raw goes `00 → 01` at capture t = 864.22 (log t = 10.76) and `01 → 00` at t = 868.25
(log t = 14.79). The log has `не нажата` at 10.01, `нажата` from 10.82 through 14.04, and
`не нажата` again at 14.85. Both edges bracketed correctly. Only two levels, so this is a
state flag, not a scaling — which is all `IDE02307` is. *Confidence: high.*

**`22D0` = coolant temperature, by elimination.** Four DIDs polled, four measurements
logged, three pinned above; `22D0` and `IDE00025` are what remain. The raw is `0xB8` = 184
and the logged value is 90.00 °C for the entire log. Under VW's usual one-byte temperature
scaling `raw × 0.75 − 48`, 184 → exactly 90.0. **But the raw never moved**, so the scaling
is *not proven* — `raw × 0.5 − 2` also gives 90.0, and so does an infinity of other lines.
What is established is the pairing (`22D0` ↔ `IDE00025`), not the codec. The value is
almost certainly the engine's coolant temperature relayed to the cluster over the
powertrain CAN, not a cluster-owned sensor.

**`0286`** was read twice, at t = 831.92 (`8E`) and t = 834.74 (`8D`). Two samples,
one decrement, no logged counterpart. **Nothing can be said.** Recorded here only so a
future session knows the DID exists.

### 4.2 Body control module `0x778` — five group identifiers, structure but no meaning

The BCM was polled with five DIDs whose responses are all a whole number of **4-byte
records**:

| DID | n | bytes | records | distinct | content |
|---|---|---|---|---|---|
| `190B` | 49 | 16 | 4 | 1 | `0224 0010 \| 013C 0010 \| 0214 0010 \| 031F 0010` |
| `1919` | 25 | 8 | 2 | 1 | `0227 0010 \| 0202 0010` |
| `192F` | 36 | 8 | 2 | 5 | `0305 aa11 \| 0201 bb11`, a ∈ {AA, AC}, b ∈ {AA, AB, AC, AE} |
| `1933` | 7 | 12 | 3 | 3 | `0147 aa11 \| 0308 bb11 \| 0139 cc11`, each ∈ {B4, B6, B8} |
| `193F` | 5 | 8 | 2 | 1 | `0206 0010 \| 022C 0010` |

Observations that hold across all five, without interpretation:

* Record length is exactly 4 bytes; the record **count** varies per DID.
* Byte 3 of every record is `0x10` or `0x11`, constant per record.
* Byte 2 is `0x00` in **every** record whose byte 3 is `0x10`, and is the **only** byte
  that ever moves in records whose byte 3 is `0x11`. That correlation is perfect over
  122 responses.
* Bytes 0–1 never moved anywhere. Byte 0 is always `0x01`, `0x02` or `0x03`.

That is enough to state a **lead**: byte 3 looks like a per-record type or validity code
and byte 2 like an 8-bit payload. It is *not* enough to name a single channel. `190B`,
`1919` and `193F` never moved at all — those three responses prove nothing whatsoever
about what they measure. The moving values step in units of 2 (`AA`/`AC`/`AE`,
`B4`/`B6`/`B8`), which rules nothing out on its own.

The capture window for these reads (t = 900…925 s in `1.jsonl`) is *after* the short drive
(t = 855…868 s) and the car was stationary throughout, which is why so little moves.

### 4.3 BCM sub-systems — two LIN slaves enumerated

The BCM answers a second family of identifiers that mirror `F187`/`F197` per sub-system:

| DID | value |
|---|---|
| `0608` | `0001 0002 FFFF FFFF …` (19 × u16) |
| `6201` | `5E1955119A ` |
| `6C01` | `WWS371 170123` |
| `6001` | `16 4D DD` |
| `6202` | `5Q0955547B ` |
| `6C02` | `RLHS         ` |
| `6002` | `00 88 5D` |

The pattern `0x6200 + n` = part number, `0x6C00 + n` = component name, `0x6000 + n` = 3
opaque bytes, with `0608` listing `0001`,`0002` and then `FFFF` padding, is internally
consistent: two sub-systems, indices 1 and 2. *Confidence: high* for the structure.

For what the two slaves *are*: VW part-number group `955` is the wipe/wash and
rain-sensor group, `5E1 955 119` is a wiper motor control unit and `5Q0 955 547` a
rain/light sensor; the component strings `WWS…` and `RLHS` are the German abbreviations
for exactly those two (Wisch-Wasch-System, Regen-Licht-Sensor). Two independent fields
agreeing is a strong lead but it rests on knowledge of VW's numbering rather than on
anything in the capture. *Confidence: good, not proven from the bus.*

Note `6001`/`6002` were each answered only after an NRC `0x78` (responsePending) — worth
allowing for in the client.

---

## 5. Negative results

These are results, not gaps in the analysis.

* **The gateway `0x77A` answered only `F187` and `F191`.** No component string, no ODX
  label name, no live data. `539B` → NRC `0x31`, five times.
* **`0x77F` (`0x715`) and `0x7DD` (`0x773`) answered nothing but session control.** Every
  DID tried on them (`2A2A`, `2A2E`, `F1B7`) returned NRC `0x31`. They are alive and
  completely unidentified.
* **`0x7B4`/`0x7B5` answered only the three address-list DIDs.** One byte each for
  `2A26`/`2A28`. Unidentified.
* **The steering column module answered no live measurement at all** — VCDS only read its
  identification block and its 2-byte coding.
* **No DTC traffic anywhere.** Service `0x19` (ReadDTCInformation) never appears in either
  capture, on any unit. Neither does `0x2E`, `0x2F`, `0x31` or security access. Both
  sessions were read-only.
* **`0600` on the BCM is 30 bytes of zeros.** Either the BCM's long coding genuinely reads
  back as zero over this DID or it is held elsewhere; the request was answered positively,
  so this is not a failure.
* **`0601`, `0606`, `0607`, `061C` → NRC `0x31` on every unit that was asked.** `0608` is
  supported only on the BCM.
* **Legacy TP 2.0 is effectively dead on this car.** VCDS sent channel-setup requests on
  `0x200` to addresses `0x1F`, `0x01`, `0x02`, `0x07`, `0x20`, `0x2A` (10–50 repeats each,
  payload `xx C0 00 10 00 03 01`). In `1.jsonl` **nothing ever answered**. In the session
  capture a single frame appeared on `0x202` — `03 C0 00 10 CA 03 01`, i.e. a channel
  offer pointing at id `0x3CA` — and VCDS never used it; no `0x3CA` frame exists in either
  file. Which unit sent it is not determined. Also unexplained: the probed addresses
  (`07`, `20`, `2A`) do not match the VCDS address of the unit VCDS opened moments later
  (cluster `17`, central electrics `09`, steering wheel `16`), so the correspondence
  between the probe address and the unit is **not** established.
* **`0x74A` was never touched in `1.jsonl`** — only in the session capture. Coverage of
  the two files is not the same; anything absent from one may just not have been asked.

---

## 6. What the next capture should record

The captures are limited by what VCDS was told to do, so the recommendation is as much
about the *operator* as about the tool. Two sessions, in this order.

### Session A — identify everything (5 minutes, stationary, ignition on)

Read the gateway list once, then walk it. This is now scriptable and needs no driving.

1. `0x710` / `2A26` — the installed-unit bitmap. Decode per §3 to get the address list.
2. For **every** request id in that list — `0x70A`, `0x70C`, `0x70E`, `0x712`, `0x713`,
   `0x714`, `0x715`, `0x746`, `0x74A`, `0x74B`, `0x767`, `0x773` (and `0x776`/`0x777`
   with the response-id caveat), plus `0x710` itself and `0x7E0`/`0x7E1`:

   ```
   10 03            enter extended session
   22 F187          part number
   22 F197          component string
   22 F19E          ODX label file  ← the join key into the label corpus
   22 F189  F191  F1A2  F1A3        version fields
   22 0600          long coding
   22 0608          sub-system list; if non-empty, read 62xx/6Cxx/60xx per index
   22 2A2E          address list
   ```

   This single pass names `0x712`, `0x713`, `0x715`, `0x746`, `0x74A`, `0x74B`, `0x767`,
   `0x773` and `0x70A` — nine units currently unknown — and fills in the gateway's own
   `F197`/`F19E`, which was never read. **This is the highest value per minute of car
   time in the whole plan.** Nothing needs to move.

3. Also run `19 02 FF` (report DTCs by status mask) on each unit. Absent from both
   captures, so its very support is unknown, and stored codes are free identification
   (a code for "left rear door contact" names the unit that owns it).

### Session B — make the candidates move (20 minutes)

The rule that killed the previous analyses applies: **a value that never changed proves
nothing.** So each identifier must be paired with something the driver *does*, at a time
that is written down. Record a VCDS log in parallel wherever a VCDS label exists for the
unit, so the arithmetic alignment of §4.1 can be repeated.

**Instrument cluster `0x714` — poll `2203`, `22D0`, `22D2`, `2B3C`, `0286` continuously.**

| do this | expected to move | why it matters |
|---|---|---|
| drive ≥ 2 km, varied speed 0–90 km/h | `22D2`, `2203` | confirms the speed factor over a real range and shows the odometer incrementing — currently both are one-point results |
| cold start, then idle to warm-up | `22D0` | **the single most valuable action.** `22D0` is stuck at `0xB8` = 90 °C in every sample we have. A cold start sweeps 10 °C → 90 °C and settles the `×0.75 − 48` vs `×0.5 − 2` question in one run. Log `IDE00025` alongside. |
| apply and release the handbrake 5+ times, some while rolling | `2B3C` | confirms it is the brake and not "vehicle stationary" |
| park in the sun, then drive — or start early morning | `0286` | outside temperature is the obvious untested candidate for a slowly-drifting byte; a 10 °C ambient swing would show |
| drive the tank from ~3/4 to ~1/4 | none known yet | **fuel level was never read.** No cluster DID in either capture varies like a tank. Scan `0x714` for unknown DIDs (§6.3) before assuming it is absent. |

**Body control module `0x70E` — poll `190B`, `1919`, `192F`, `1933`, `193F` continuously.**
Three of the five never moved at all. The point of this session is to make them move, one
action at a time with 10 s of quiet either side so the edge is unambiguous:

| do this, one at a time | which record byte to watch |
|---|---|
| left indicator 10 cycles, then right 10 cycles | any byte-2 that toggles at 1.5 Hz; different counts for left vs right separate the two |
| brake pedal press/release ×10 | a byte that follows the pedal exactly |
| driver door open/close, then each other door in turn | four distinct bits/bytes, one per door — the classic BCM signal set |
| headlights off → side → dipped → main | a byte with 4 levels |
| wipers off → int → slow → fast, then the washer | the wiper sub-system (`6C01` = `WWS371`) is on this unit |
| cover the rain/light sensor with a cloth, then uncover | the `RLHS` sub-system (`6C02`); a light-level channel should sweep |
| lock and unlock the car with the remote | a central-locking state byte |
| turn the ignition off → on → crank | terminal 15/50 status; also the only way to see a proper battery-voltage sweep |

The last one is worth calling out: `192F` and `1933` move in steps of 2 around `0xAA`–`0xAE`
and `0xB4`–`0xB8`. Cranking the starter drops battery voltage by 2–3 V for a second. If
those bytes plunge during crank, they are voltage; if they do not, they are not. That is a
one-second experiment that discriminates decisively, which is exactly the kind of test the
current data cannot supply.

**Steering column module `0x70C`** — no live identifier is known at all. Sweep the wheel
lock-to-lock (see §6.3), then repeat with the indicator stalk, the wiper stalk and the
horn. Steering angle is the signal most likely to be here and it is trivially sweepable.

### 6.3 A blind DID sweep is now cheap and should be done

Every unit in §1 answers NRC `0x31` for unsupported identifiers and a positive `0x62` for
supported ones, promptly and without a session drop. A sweep of `0x0000..0xFFFF` per unit
at ~100 requests/s is under 11 minutes per unit, and the ranges actually in use are
narrow: `0x02xx`, `0x06xx`, `0x19xx`, `0x20xx`–`0x22xx`, `0x2Axx`, `0x2Bxx`, `0x38xx`,
`0xF1xx`, `0xF4xx`. Sweeping just those nine pages is ~2 300 requests, under a minute.

Do it **twice** on each unit — once parked and once mid-drive — and diff the answers. The
identifiers whose bytes differ between the two runs are the live measurements, and that
list is obtained without needing the label corpus at all. Restrict any later fitting to
that list; it removes most of the surface on which a false positive can form.

### 6.4 Housekeeping for the capture itself

* Keep the wall-clock `Marker` — the whole of §4.1 depends on it.
* Have VCDS log in parallel for any unit where a label exists, and note the *action* times
  in a third file (even a phone voice memo with timestamps). The cluster result worked
  because there was a CSV to align against; the BCM result did not, because there was not.
* Record one unit at a time. In `1.jsonl` VCDS round-robins across DIDs at ~5 Hz per unit,
  which is fine, but interleaving units halves the sample rate for each.
* The label corpus at `research/vcds-data/{en,rus}` is a symlink to an unmounted Windows
  volume and was **not** available for this analysis. Mounting it would let the five known
  `F19E` names — `EV_SMLSVALEOMQBLRH`, `EV_BCMMQB`, `EV_DashBoardVDDMQBAB`,
  `EV_ECM18TFS0208V0906264H`, `EV_TCMDQ200021` — be resolved against the `.rod` files,
  which is the intended join and would name the BCM's group records without any of the
  guessing in §4.2.
