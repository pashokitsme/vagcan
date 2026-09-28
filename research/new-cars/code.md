# New cars — assumptions in the code (2026-09-28)

> Later the same day, `survey.rs` was removed with `dev survey` (`feat/units-without-survey`,
> `todo/label-lookup/03`); the `survey.rs:…` citations below are against `db40ed6` and no
> longer resolve. The units a car has are recorded by `watch`, `measure` and `units --identify`
> (`vag-cli-core/src/units.rs`) instead.

What in **this codebase** would break, or silently misbehave, on a VAG car newer than the
reference Škoda (≈ 2020 on: MQB-evo, MEB, MLB-evo, PPE), and what fixes each. Research only:
no code was changed, and no car, adapter or BLE was touched.

Two sibling notes cover the rest of the owner's question: the label data is
[`labels.md`](labels.md); transport and protocol — SFD, DoIP, CAN FD, 29-bit ids as a fact
about the car — is [`transport.md`](transport.md). A finding here that rests on one of their
facts is written as a conditional ("**if** the car answers on 29-bit ids, …").

Evidence is `file:line` against `master` at `db40ed6` (paths shortened to the crate once the
crate is clear). A claim about what a newer car does is **sourced** (§13), or marked
**inferred**. "Breaks" means: a newer car gets a wrong answer, a refusal, or nothing, where the
reference car gets the right one.

## 0. Summary

| # | assumption | `file:line` | breaks on newer cars? | confidence | fix | cost | needs a car? |
|---|---|---|---|---|---|---|---|
| A1 | a unit's CAN ids are 11-bit `u16` in every layer above the frame | `vag-uds-client/src/address.rs:35-38`, `schedule/mod.rs:99-102`, `guard.rs:331`, `vag-dash-render/src/plan.rs:180-190,239` | **yes, if** the car answers on 29-bit ids | high (code) | `UnitAddress`/`schedule::Unit`/plan units hold `CanId` (or raw `u32` + extended bit) | M, mechanical, ~8 crates | one 29-bit car |
| A2 | exactly two answer rules: `7E0..7E7 → +8`, `700..7BF → +6A` (and only up to `795`) | `address.rs:40-72` | **yes, if** 29-bit, or for a unit past `795` | high (code) | an addressing *scheme* per car: logical address → id pair; chosen by one read at the gateway, saved per VIN | M | yes |
| A3 | a VCDS unit number is one byte; 2 hex digits = number, 3+ = an 11-bit CAN id | `address.rs:116-128,321-357`, `vag-data-labels/src/label.rs:31-53`, `vag-cli-core/src/dash.rs:146-151` | **yes**: a Golf 8 lists `8107 C002 C003`, an ID.3 `8105`–`813F` — they cannot be typed, installed or put in `dash.toml` | high (sourced + code) | unit numbers `u16`; CAN ids spelled unambiguously (`id:714` or by width) | S | no |
| A4 | every command hand-builds `CanId::Standard`; the laptop's bus refuses anything else | `vag-cli-core/src/bus/channel.rs:27-31,46-47`, `bus/task.rs:270`, `survey.rs:444,496`, `faults.rs:441,491`, `units.rs:46,71,115`, `vag-cli/src/main.rs:889-894,1009,1042`, `safety.rs:39`, fw `dash.rs:1709` | **yes, if** 29-bit | high (code) | one `UnitAddress::ids() -> (CanId, CanId)`; the ~12 hand-built pairs go | S after A1 | no |
| A5 | the host ↔ board link carries 11-bit ids; `> 0x7FF` is malformed | `vag-uds-transport/src/link.rs:20-22,198,223-231,244-255,432-438` | **yes, if** 29-bit: the dash cannot address the car over USB or BLE | high (code) | link v2: `u32` ids + extended bit, announced in `HelloReply` | S–M | no (bench) |
| A6 | the board's acceptance filter is one 11-bit code/mask | `vag-uds-can/src/filter.rs:15-106`, fw `dash.rs:95,1622-1674,1882-1886` | **yes, if** 29-bit: answers filtered out — silence, not an error | high (code + esp-hal's own doc) | an extended twin of `StandardFilter` → esp-hal `SingleExtendedFilter` | S | bench, then car |
| A7 | five reference-car pairings are the built-in fallback (`09→70E`, `16→70C`, `17→714`, `01→7E0`, `02→7E1`) | `address.rs:170-192` | maybe: MQB facts, unproven on newer platforms, wrong under a 29-bit scheme | medium | keep the ISO pair; learn the rest from the car (`units --identify` already does) | S | no |
| A8 | offline analysis pairs requests and answers for the 11-bit rules and ISO `18DA` only | `vag-cli-core/src/analyse.rs:142-161` | **yes, if** VW 29-bit: a capture sniffs fine and analyses to nothing | high (code) | use A2's scheme | S | no |
| B1 | the gateway is `0x710`; two call sites spell it again | `gateway.rs:38-45`, `units.rs:45`, `vag-cli/src/main.rs:1008` | maybe; **yes, if** 29-bit | medium | gateway = logical `0x10` under the car's scheme | S | yes |
| B2 | the list is `22 2A26` only; `04A3` (same bytes on the reference car) is never tried; `vagcan units` fails without the list | `gateway.rs:23-36`, readers `units.rs:47`, `main.rs:1012-1015`, `faults.rs:442`, `survey.rs:445`, `faultcount.rs:204` | maybe (newer gateways: `GW2020`, `ICAS1`) | medium | `2A26`, then `04A3`; `units` degrades like `faults` | S | yes |
| B3 | the walk is exactly the `2A26` bitmap, bit `n` = request `0x700 + n`; the board decodes 24 bytes | `gateway.rs:71-100`, `faultcount.rs:339-352` | maybe: a unit on a 29-bit id, or listed elsewhere by a 2020+ gateway, is never walked and nobody is told | low–medium | decode to logical addresses; check the list against VCDS's unit count (ID.3: 35, Golf 8: 24) | S–M | yes |
| B4 | bits `0x00`, `0x76`, `0x77` are walked (laptop) or skipped as shared ids (board) | `gateway.rs:57-69,85-89`, `faultcount.rs:282-292` | maybe — `0x76`/`0x77` may be units the reference car already has and nothing can reach | low (inferred) | the scheme decides their ids; skip bit 0 | S | yes (two reads) |
| C1 | at most 64 units; the board's fault count refuses a longer walk | `guard.rs:114-124,612`, `faultcount.rs:66-73,294-296` | probably no: a Golf 8 lists 24 units, an ID.3 35 | medium (sourced for two cars) | size from `ram-budget.sh`; count what fits instead of refusing all | S | no |
| C2 | `SURVEY_RANGES`, the `--blind` default, is this Škoda's pages | `survey.rs:47-63` | no (only a default), but a one-car constant | high | derive from the unit's declared pages | S | no |
| D1 | classic ISO-TP only: 8-byte frames, SF length in a nibble, 7-byte CFs | `vag-uds-can/src/isotp.rs:10-11,58-61,125-233` | **yes, if** the OBD CAN is FD (fails as silence) | high (code) | ISO 15765-2:2016 FD framing | M | FD adapter + car |
| D2 | no first-frame escape: answers over 4095 bytes are a protocol error | `isotp.rs:133-135,189-195`, `link.rs:195-196` | maybe: a `19 0A` from a unit with > 1,023 codes | low | the escape (~30 lines) on the laptop; the board keeps its heap bound | S | no |
| E1 | ISO 14229-2 default waits everywhere; a 300 ms identification probe | `bus/mod.rs:54-93`, `units.rs:121-130`, fw `dash.rs:1540-1600`, fw `faults.rs:59` | probably no; the probe is the tightest wait and drops a slow unit silently | low (inferred) | per-unit P2/P2* from the project; retry the probe once at P2* | S | yes (to observe) |
| F1 | a unit identifies itself by `F187`/`F197`/`F19E`/`F1A2` | `units.rs:121-146`, `survey.rs:65-75` | no: Golf 8 and ID.3 units show `<ODX name> <6-digit version>` and `<name>_<vvv>.rod` files, the shape the code joins | high for the shape (sourced); the VCDS-line ↔ `F19E`/`F1A2` mapping inferred | — | — | no |
| F2 | a `Family` variant match (no `_<vvv>` file) is used without saying so | `vag-data-labels/src/label_files.rs:195-218`, `vag-cli-core/src/extracted.rs:344-370` | maybe: the ID.3 gateway (`EV_GatewICAS1MEB` `005001`) only has a `Family` file | high (code) | show "variant not confirmed" per unit | S | no |
| F3 | the VIN comes from `0x7E0` only; `info` reads `01` and `02` only | `units.rs:58-85`, `vag-cli/src/main.rs:897-911` | maybe: a BEV's `01` is a vehicle control unit and it has no `02` | medium | VIN: engine, then gateway, then any listed unit; `info` from the walk | S | no |
| F4 | the label project is not chosen by the car (`covering()` is always `None`) | `vag-cli-core/src/project.rs:200-234,257-274` | maybe: with two projects on disk a newer car is read against the last one set up (raw bytes, not wrong numbers) | high (code) | fill `covering()` from the project's own coverage | M | no |
| G1 | 500 kbit/s classic CAN, adapter opened straight in `Normal` | `vag-cli-core/src/device.rs:552-567`, `vag-uds-can/src/slcan.rs:16-91` | no on classic CAN; **yes, if** FD | high (code) | FD adapter/backend (hardware, `transport.md`); open listen-only first | M + hardware | yes |
| H1 | the board's TWAI runs `Normal` from power-on at 500 kbit/s | fw `dash.rs:1866-1887`, `vag-dash-fw/src/can.rs:18-25` | **yes, if** FD — and a classic node in `Normal` on an FD bus disturbs it | medium (inferred) | start listen-only, go `Normal` only on a clean bus | S | bench + one FD car |
| I1 | one exchange in flight; fixed rates; 8 identifiers a request | `schedule/mod.rs:30-35,69-80,133-194` | no: correct on any CAN car; the 8 is learned down per unit | high | — | — | no |
| J1 | road speed = `22 F40D` on `0x7E0`, one byte, J1979 | `guard.rs:141-153,294-302`, `remote.rs:119-123`, `vag-cli-diag/src/safety.rs:17-58` | maybe: on a BEV `01` is a vehicle control unit; whether it serves J1979's `F40D` is not known | medium | resolve a speed source from data (vehicle-speed row), carried in the dash plan; `0x7E0`/`F40D` first | M | yes (one BEV) |
| J2 | the laptop reads the **first byte** of any-length `F40D` answer | `safety.rs:29-31`, `pdu.rs:90-100` | maybe, **unsafe**: a two-byte ×0.01 answer at 25.6 km/h reads as 0 km/h, "stationary" | high (code) | reuse `guard::road_speed`, which insists on one byte | S | no |
| J3 | with no speed, a *parked* survey or identification is refused and the text points at `--while-driving` | `survey.rs:421-431`, `vag-cli/src/main.rs:1122-1142` | yes, wherever J1 fails | high (code) | say the check cannot run on this car; do not offer the override as the way out | S | no |
| K1 | `measure` requires "engine speed", "selected gear", "accelerator pedal position", by English words | `vag-cli-measure/src/channels.rs:189-300,429-465` | **yes** on a BEV (inferred); maybe with a non-English project | medium | roles by VW's measurement ids (`IDE…`), words as fallback; required per drivetrain | S–M | yes (one BEV) |
| L1 | fault record `0x01` has the reference car's layout; parsed from any record ≥ 13 bytes | `vag-uds-client/src/dtc.rs:44-199`, `vag-cli-diag/src/faults.rs:589-604` | maybe, silently: a confident wrong mileage and date | medium | record `0x01` only, length exactly 13; layout from the project | S | yes (one newer unit) |

**In short.** Nothing in the code is tied to the Škoda's *identifiers* any more; it is tied to
its **addressing**. Every layer above the CAN frame names a unit by its 11-bit request id
(A1–A6), and VCDS's own scans show 2020+ cars with unit numbers the tool cannot even spell
(A3). One addressing type — logical address plus scheme, with the VCDS number as a separate
name — fixes most rows. Two rows are worth fixing whatever else is decided: the laptop's speed
decoder (J2) and the survey's refusal text (J3).

## 1. Addressing — 11-bit ids, the `0x7xx` block, answer-id rules

**What the code assumes.** A control unit is `UnitAddress { request: u16, response: u16 }`
(`crates/uds/vag-uds-client/src/address.rs:35-38`). `from_request` knows two rules and no
third (`address.rs:65-72`):

- ISO 15765-4's block, `0x7E0..=0x7E7`, answers on request `+ 8` (`address.rs:41-44,67`);
- VW's block, `0x700..=0x7BF`, answers on request `+ 0x6A` (`address.rs:48-50,68`), and only
  while the answer fits eleven bits, so requests past `0x795` get `None`
  (`address.rs:52-56,71`).

Both rules come from captures of the reference car (`address.rs:12-14`). The 11-bit shape is
repeated, not derived, in every layer above the frame:

| layer | where | what it holds |
|---|---|---|
| scheduler | `vag-uds-client/src/schedule/mod.rs:99-102` | `Unit { request: u16, response: u16 }` |
| laptop bus | `vag-cli-core/src/bus/channel.rs:27-31,46-47` | an extended id becomes `unit: None`; `send` returns `Unsupported("extended CAN ids on the bus scheduler")` |
| laptop bus task | `vag-cli-core/src/bus/task.rs:270` | `to_unit(CanId::Standard(..), CanId::Standard(..))` |
| commands | `survey.rs:444,496`, `faults.rs:441,491`, `units.rs:46,71,115`, `vag-cli/src/main.rs:889-894,1009,1042`, `safety.rs:39` | hand-built `CanId::Standard` pairs |
| BLE guard | `guard.rs:331,391,603` | units keyed by `u16` request id |
| fault count | `faultcount.rs:120,131,311-316` | `u16` request ids |
| host ↔ board link | `vag-uds-transport/src/link.rs:20-22,198,432-438` | `request_id u16, response_id u16`; over `0x7FF` is `Malformed("CAN id over 11 bits")` |
| board firmware | `vag-dash-fw/src/bin/dash.rs:1709` | `IsoTpCan::new(backend, CanId::Standard(unit.request), …)` |
| board filter | `vag-uds-can/src/filter.rs:20-63`, fw `dash.rs:1632-1643` | one 11-bit code/mask (`SingleStandardFilter`) |
| dash plan | `vag-dash-render/src/plan.rs:180-190,239`; answer id from `from_request` at build, `vag-cli-core/src/dash.rs:1570` | `Unit { request: u16, response: u16 }`, `Channel.unit: u16` |
| per-car files | survey `survey.rs:212` read by `vag-cli-core/src/plan.rs:281-287`; car file `vag-cli-measure/src/carfile.rs:418`; favourites `vag-cli-diag/src/watch/favourites.rs:127` | ids written `{:03X}`, read with `u16::from_str_radix`; `plan.rs:284` drops a line it cannot parse, silently |
| command line, `dash.toml` | `address.rs:321-357`, `vag-cli-core/src/dash.rs:146-151` | two hex digits are a VCDS number, three or more an 11-bit id |

**Below that line the stack already carries 29-bit ids.** The frame layer uses SocketCAN's
raw form, bit 31 = extended (`vag-uds-can/src/backend.rs:7-30`); `IsoTpCan::new` takes a
`CanId` and is tested with `0x18DA10F1` (`vag-uds-can/src/isotp.rs:36-42,460-464`); slcan
encodes and decodes `T` lines (`vag-uds-can/src/slcan.rs:50-54,78`), and so does the board's
`slcan` image (`vag-dash-fw/src/slcan.rs:799,979-1015`); the firmware's TWAI backend converts
both id kinds (`vag-dash-fw/src/can.rs:188-207`); captures store a `CanId`
(`vag-uds-capture/src/record.rs:11-13`). `dev sniff` already recognises VW's 29-bit pairs —
`is_diag_id` accepts `0x17FC_00xx`/`0x17FE_00xx` "MQB gateways use"
(`vag-cli-diag/src/sniff.rs:21-37`). That comment has no source (it came with `acfad99`,
2026-07-31), and `arrow` gives those ids no direction (`sniff.rs:44-60`).

**VCDS numbers wider than a byte (A3), sourced.** VCDS Auto-Scans posted on Ross-Tech's forum
show 2020+ cars with four-digit unit addresses:

- a Golf 8 (`CD-VW38`): `01 03 05 08 09 10 13 15 17 19 23 2B 3C 42 44 52 5F 75 A5 D6 D7
  8107 C002 C003` (thread 40985, VCDS 23.11);
- a 2020 ID.3 (`E1-VWE3`): 25 one-byte addresses plus `8105 8107 811E 811F 8120 8121 8123
  8124 8125 813F` — DC/DC converter, antenna, Kessy sensors, the ICAS1 application servers,
  the driver display (thread 24641). An ID.3 was scanned the same way through a HEX-V2
  (thread 25398: "VCDS Version: 20.9.1.0 HEX-V2"), and VCDS speaks DoIP on HEX-NET2 interfaces
  only (revision history, 20.4) — so these units answer through the OBD port's **CAN**, on ids
  this note cannot see.

In `vagcan` a unit number is `u8` everywhere: `UnitNumber.number` (`address.rs:118-122`),
`UnitLabel.address` parsed with `u8::from_str_radix` (`vag-data-labels/src/label.rs:33-35,50`,
so a `(#8105)` in a label file is skipped by `.ok()?`), and `parse` reads `8105` as a CAN
request id and refuses it with "has no diagnostic address (700-795 or 7E0-7E7)"
(`address.rs:333-335`). The same parser reads `dash.toml`'s `<unit>:<text id>`
(`dash.rs:151`). So on those two cars three (Golf 8) and ten (ID.3) units cannot be named at all
— before any question of which CAN id they answer on. The VCDS number is only a name: on the
reference car VCDS `17` is request `0x714` and `19` is `0x710` (`address.rs:183-187`), so a
four-digit number says nothing yet about the unit's CAN id.

**How it breaks.** *If the car answers on 29-bit ids* (`transport.md` §4), every layer in the
table fails closed: `from_request` gives no address, the laptop's bus refuses the id, the
board's link rejects the message as malformed, and the board's 11-bit filter never hands the
answer up (esp-hal: "Extended IDs that match the bit layout of this filter will also be
accepted" — by accident of leading bits, `esp-hal-1.0.0-rc.0/src/twai/filter.rs:95-99`; no
`0x7xx` code matches a `0x17FE…` id). The user sees "has no diagnostic address" or silence,
never a wrong value. The fault is not the frame layer: a unit's *identity* is its 11-bit
request id everywhere above it. The `+0x6A` rule's own edge is already a known gap —
`0x796..=0x7BF` are counted `unaddressable`, never asked (`faultcount.rs:142-148,275-281`).

**The fix (design sketch).**

1. **A unit is a logical address plus a scheme; the scheme yields the CAN ids.** VW's own
   numbering already works that way: the reference car's cluster is UDS address `0x14` on
   request `0x714`, the gateway `0x10` on `0x710` (`address.rs:183-187`). So
   `enum Scheme { Iso11 /* 7E0+n, +8 */, Vw11 /* 700+la, +6A */, Vw29 { req: u32, resp: u32 }, Iso29 /* 18DA ta sa */ }`
   and `UnitAddress { logical: u16, scheme: Scheme }` with `fn ids(&self) -> (CanId, CanId)`.
   The logical address is what a scheme maps to ids (`0x14` → `0x714`); it is `u16` only so a
   29-bit scheme with a wider field fits. The VCDS number (`17`, `8105`) stays a separate,
   data-fed name for it, widened to `u16` (A3). `Vw29`'s bases are parameters until `transport.md` or a capture establishes them;
   `sniff.rs:27-29` is where today's candidates are written. In `vag-uds-client`, `no_std`-clean,
   ~300 lines with tests. A8 (`analyse.rs`) and `sniff`'s `arrow` use the same type.
2. **`schedule::Unit`, the guard's keys, the fault count and the dash plan hold `CanId`** (or a
   `u32` in the raw form `backend.rs:7-13` defines). Mechanical: the planner only compares ids
   (`schedule/mod.rs:94-102`). Board RAM: +2 bytes per id in `Unit` and per plan channel — tens
   of bytes; run `ram-budget.sh` before and after, as `CLAUDE.md` requires.
3. **Which scheme a car uses is read, not configured.** Ask the gateway `22 F187` once under
   each scheme the tool knows — a handful of reads, not a sweep — keep the first that answers,
   and save it under the VIN beside the survey cache. A car that answers under none is told so.
4. **The host ↔ board link gets a v2 body** with `u32` ids and the extended bit
   (`link.rs:20-22`), announced in `HelloReply` (`link.rs:26`); the laptop refuses to address a
   29-bit unit through a v1 board rather than truncating.
5. **The board's filter follows the id kind.** `StandardFilter` gains an extended twin and
   `FilterFollower` holds either (`filter.rs:65-106`); esp-hal has `SingleExtendedFilter`
   (`esp-hal-1.0.0-rc.0/src/twai/filter.rs:228`), and the firmware imports only the standard
   one (`dash.rs:95`).
6. **Spelling.** Unit numbers are `u16` hex (`8105` is a number); a CAN id is written so it
   cannot be one (`id:714`, or 8 digits for 29-bit). Per-car files write ids at their own width
   and keep reading today's three-digit form.

Cost: M overall — one new type, then a mechanical sweep; only step 3 needs a car.

**Built-in pairings (A7).** `BUILT_IN_SHORT_NUMBERS` (`address.rs:192`) is five pairings
proven on the reference car. `01→7E0` and `02→7E1` follow ISO 15765-4's convention; `09→70E`,
`16→70C`, `17→714` are MQB facts observed on one car (`address.rs:176-181`). They sit under the
label files and the override file (`address.rs:283-299`), so on a newer MQB-evo car they are
*probably* still right (**inferred** — nothing seen shows VW moving those units) and on a
29-bit car they are wrong ids. Keep the ISO pair; drop the three VW ones and rely on the path
that exists — the gateway list plus `F187` looked up in the label files (`address.rs:187-190`).

**Not known.** Which CAN ids the four-digit units answer on (`transport.md` §4). Whether any
2020+ VAG car answers on 29-bit ids at the OBD port. Whether the one-byte logical addresses
(`0x10` gateway, `0x14` cluster, …) are unchanged on MQB-evo / MEB / PPE (**inferred** yes).

## 2. The gateway — `0x710`, `2A26`, the bitmap, `walk_order`, `NOT_LISTED`

**What the code assumes.**

- The gateway answers on request `0x710` (`vag-uds-client/src/gateway.rs:38-45`), called "a
  property of VW's platform, not of one car" — the evidence is one car's `3Q0 907 530 B`
  (`gateway.rs:40-43`). Two call sites spell `0x710` again instead of using the constant:
  `vag-cli-core/src/units.rs:45`, `vag-cli/src/main.rs:1008`.
- The installation list is `22 2A26` (`gateway.rs:23-24`). `04A3` returned the same 32 bytes on
  the reference car and is defined (`gateway.rs:26-27`) but **no caller tries it**; `2A28` is a
  subset of unknown meaning (`gateway.rs:29-36`). Every reader asks `2A26` alone: `units.rs:47`,
  `vag-cli/src/main.rs:1013`, `faults.rs:442`, `survey.rs:445`, `faultcount.rs:204`.
- Bit `n`, least significant first, is the unit at request `0x700 + n` (`gateway.rs:77-100`).
  The archive's reading is narrower: the bitmap is "indexed by request-id low byte", and the
  `0x700 +` is the 11-bit rule applied on top (`.archive/research/car/other-ecus.md` §3,
  lines 171-181).
- Bit 0 (`0x700`) is set on the reference car; `0x700` is where VCDS broadcasts TesterPresent
  and nothing answers (`other-ecus.md` lines 31-32). The walk asks it every time — one timeout
  per walk.
- Bits `0x76`/`0x77` are listed, and `0x776`/`0x777` are `0x70C`/`0x70D`'s answer ids, whose
  `+0x6A` lands on the engine's and the gearbox's request ids. The archive calls it an open
  problem — "do not extend the rule past `0x773`" (`other-ecus.md` lines 194-198). The laptop
  tries them (`gateway.rs:60-63,85-89`); the board skips them as `SharedId`
  (`faultcount.rs:40-42,282-292`).
- The laptop decodes every byte of the answer (`gateway.rs:90-100`), the board only the first 24,
  `0x700..=0x7BF`, counting the rest as `past_block` (`gateway.rs:71-75`, `faultcount.rs:339-352`).
- Three units are walked whether listed or not: `0x7E0`, `0x7E1`, the gateway
  (`gateway.rs:47-51,57-69`); `units.rs:50-52` adds `0x7E1` itself for `watch`/`measure`.

**How it breaks on a newer car.**

1. **Newer gateways are new units.** The Golf 8's is `EV_GatewMQB2020` (`GW2020 High`), the
   ID.3's `EV_GatewICAS1MEB` (`ICAS1 Host-SG`) — sourced, §13. Whether either answers `2A26`
   is not known. *If the list is not there*, every whole-car command loses it, and they degrade
   differently: `faults` and `dev survey` print a line and walk only `NOT_LISTED`
   (`faults.rs:444-447`, `survey.rs:447-455`); `watch`/`measure` identify what they were asked
   for plus `0x7E1` (`units.rs:47-52`); `vagcan units` **fails outright** (`main.rs:1012-1015`,
   `.context(…)?`); the board's badge is `?` (`faultcount.rs:28-29`).
2. *If `2A26` is refused but `04A3` answers*, the same happens although the tool knows `04A3`.
3. **Maybe: units the list does not hold.** The bitmap is indexed by CAN-id low byte, not by
   VCDS number, so a four-digit VCDS address (§1) does not by itself keep a unit out of it — an
   `8105` on an 11-bit id is one more bit. But a unit answering on a 29-bit id, or listed by a
   2020+ gateway somewhere other than `2A26`, is never walked, and nothing tells the user: the
   walk is exactly the list. The check is cheap: the ID.3's Auto-Scan shows 35 units, the Golf
   8's 24 (§13); a `vagcan units` that finds fewer on the same car has missed some.
4. *If a unit's one-byte address is past `0x95`*, the 11-bit rule cannot address it: the laptop
   prints "has no diagnostic address … skipped" (`faults.rs:486-490`, `survey.rs:489-495`), the
   board counts it `unaddressable` (`faultcount.rs:275-281`), and bits `0xC0..=0xFF` the board
   does not decode at all. Under another scheme those bits are ordinary units.
5. **The two unresolved bits may already be this.** `0x76`/`0x77` are exactly where the
   11-bit rule stops working, and `sniff.rs:27-29` says MQB gateways use `0x17FC00xx`/
   `0x17FE00xx`. A reading that fits both: the units at logical `0x76`/`0x77` answer on VW's
   29-bit pair. **Inferred**, untested; if true, the reference car has two units nothing in
   `vagcan` can reach.
6. Cars without `0x7E0`/`0x7E1` spend a timeout on each per walk. The ID.3 and the Golf 8 above
   list no address `02` (§13). Harmless.

**The fix.**

- **Decode the list to logical addresses, all 32 bytes, and let the car's scheme (§1) give each
  its ids.** The board's guard stays a bound on how many it *asks* (§3), not on how much it
  *decodes*: 32 bytes are a fixed 256-bit set, no allocation driven by the answer.
  `vag-uds-client/src/gateway.rs` + `faultcount.rs`, ~100 lines.
- **Check the list against VCDS's count** on one 2020+ car — every unit its Auto-Scan shows
  should be a bit. If some are not, find the list that holds them: one `survey` of the gateway
  alone (its declared identifiers include its lists) or the gateway's ODX in `labels.md`'s
  sources. Then decode that too. M.
- **Try `2A26`, then `04A3`,** before declaring "no list".
- **The gateway is logical `0x10` under the car's scheme**, one constant; the two hand-spelled
  `0x710`s go.
- **Skip bit 0**, and stop special-casing `0x76`/`0x77` once the scheme decides their ids.
- **`vagcan units` degrades like `faults`** instead of failing.
- **With no list at all**, the fallback is the label data, not a sweep: the project names the
  units a vehicle can have (`labels.md` §2), and asking each once for `F187` is what
  `units --identify` does per listed unit today. Guarded as `survey` is (§3): a list of
  candidates asked blind is a sweep of addresses.

Cost: S–M. Needs a car: yes — one `vagcan units` on a 2020+ car, and for item 5 one `22 F187`
at `0x17FC0076` (a single read).

**Not known.** Whether `GW2020` / `ICAS1` gateways answer `2A26`, and whether it holds every unit
VCDS shows on those cars. What `2A28` means.

## 3. Unit count and sweep guards — `MAX_UNITS`, the `0x795` line, `survey`

**What the code assumes.**

- `guard::MAX_UNITS = 64` units remembered at once by the board's guard, radio and cable
  (`vag-uds-client/src/guard.rs:114-124`, refusal at `guard.rs:612`), sized for the board's heap
  — "under 10 KB worst case" (`guard.rs:118-123`).
- The board's fault count uses the same number and **refuses a longer walk whole**
  (`faultcount.rs:66-73,294-296`): "a car lists fifteen to twenty" and more is "not a car's but
  the block's" (`faultcount.rs:67-72`).
- The `0x795` line (§1): ids past it are never asked, anywhere.
- The laptop's `faults` and `dev survey` have **no** unit cap; they walk whatever the list names
  (`faults.rs:437-453`, `survey.rs:440-461`). `survey` is guarded otherwise — declared
  identifiers only, `--blind` per named unit, refused on a moving car unless
  `--while-driving`, stopped whole on an anomaly (`survey.rs:15-25,336-356,409-432`).
- `SURVEY_RANGES`, the `--blind` default, is "the pages *this* Škoda was seen using"
  (`survey.rs:47-63`) — flagged as one car's answer in the code itself.

**How it breaks on a newer car.**

- **Probably no: 64 units.** The Golf 8 above lists 24 addresses and the ID.3 35 (§13) — more
  than the reference car's 15, and well under 61 (64 less the three always walked). Large
  MLB-evo / PPE cars are not measured here. "Fifteen to twenty" in the comment is already out
  of date.
- **No: the `survey` guards, nor the radio guard's sweep rules** (4 identifiers a request, 32
  distinct a unit, 8 evenly spaced make a walk — `guard.rs:103-108`). They key on what a unit's
  own data declares, on what a host asks, and on the moving-car check — not on this car's ids.
  The exception is inherited: where the moving-car check cannot answer, a *parked* survey is
  refused too (§10, J3).
- `SURVEY_RANGES` breaks nothing — it is only the default of a sweep somebody aims by hand — but
  on a newer car it sweeps pages nobody has evidence for, and it is exactly a one-car constant.

**The fix.**

- Keep a cap and make it the board's measured memory budget, not a guess about cars; let the
  fault count report what it counted up to the cap (a badge that says "partial") instead of
  refusing everything past it. S, `faultcount.rs` + `guard.rs`; `ram-budget.sh` before and after.
- Derive the blind default from the pages the unit's own label data names. S.

**Not known.** Unit counts on MLB-evo / PPE flagships.

## 4. ISO-TP — frame size, flow control, lengths

**What the code assumes.** The one live ISO-TP is `IsoTpCan` (`vag-uds-can/src/isotp.rs`),
used by the laptop through `vag-uds-can/src/link.rs:43-45` and by the board at fw
`dash.rs:1709`. The sync `SoftwareIsoTp` (`vag-uds-client/src/isotp.rs`) is reached only from a
replay test (`vag-uds-client/tests/e2e_replay.rs:78`) and is left out.

| assumption | where | standard or car? |
|---|---|---|
| classic CAN: every frame padded to 8 bytes with `0x00` | `isotp.rs:10-11,58-61,130,160,202` | ISO 15765-2 classic; the padding value is the implementer's |
| single-frame length is the low nibble; `SF_DL = 0` reads as an empty PDU | `isotp.rs:181-188` | classic only — in FD framing `0x00` escapes to a length byte |
| first frame: 12-bit length, `FF_DL ≤ 7` is an error, no 32-bit escape | `isotp.rs:133-135,189-195` | ISO 15765-2:2016 allows the escape past 4095 bytes |
| consecutive frames carry 7 bytes | `isotp.rs:156-160,219-223` | classic only |
| our flow control: `30 00 00` — BS 0, STmin 0 | `isotp.rs:201-203` | standard |
| N_Bs 1000 ms, N_WFTmax 8 | `isotp.rs:12-15` | standard defaults |
| a PDU is at most 4095 bytes, on the board's link too | `isotp.rs:133`, `link.rs:195-196` | 12-bit length |
| the board forwards requests of at most 64 bytes | `guard.rs:97-102` | a heap bound, not a bus fact |

**How it breaks on a newer car.**

- *If the diagnostic CAN is FD* (`transport.md` §3): an FD single frame reads as an empty PDU,
  which `schedule::answers` drops as a late answer (`schedule/mod.rs:297-313`), so the unit looks
  silent; an FD first frame is reassembled 7 bytes per consecutive frame and fails its sequence
  check. Nothing wrong reaches the screen — it fails as "no answer". The passive sniffer knows
  the escape and says so (`vag-uds-can/src/sniff.rs:112-119`). Neither adapter carries FD frames
  anyway (§7, §8), so this layer is second in line.
- **Maybe, on classic CAN too: an answer over 4095 bytes.** `vagcan` calls the escape a protocol
  error (`isotp.rs:193-195`). Only a very long answer needs it: `19 0A` (every supported code,
  `faults --supported`, `uds_async.rs:105-114`) is `3 + 4n` bytes, so more than 1,023 codes. The
  reference car's body control module answers 508 codes under mask `0xFF`
  (`faultcount.rs:18-23`); a bigger newer one is **inferred** possible, not observed.
- Padding `0x00`, BS 0, STmin 0 and N_Bs are standard choices. **No.**

**The fix.** FD framing (SF and FF escapes, CFs up to 63 bytes, DLC steps) in `IsoTpCan`, behind
a frame size the backend reports: ~200 lines in `vag-uds-can`, tested against ISO 15765-2:2016's
examples. The FF escape alone is ~30 lines and useful on classic CAN. The board keeps its 4095-
and 64-byte limits — heap bounds that refuse a longer answer by design.

**Not known.** Whether any 2020+ VAG car's OBD-port CAN is FD. The ID.3 answered a HEX-V2
(§1), which is not a DoIP interface (revision history, 20.4); that it has no CAN FD either is
**inferred**, so the 2020 ID.3's diagnostic CAN is probably classic. Largest real answer on a
newer car.

## 5. UDS timing — P2, P2*, `78`

**What the code assumes.** ISO 14229-2 defaults, none per unit:

| wait | value | where |
|---|---|---|
| a scheduled read's answer | 500 ms (10 × P2 of 50 ms) | `vag-cli-core/src/bus/mod.rs:54-60` |
| after `7F xx 78` | 5 s (P2*), at most 30 in a row | `bus/mod.rs:62-64,90-93`, loop `bus/task.rs:292-321` |
| a suppressed-positive request (`3E 80`, `10 81`) | 150 ms (3 × P2) | `bus/mod.rs:66-72` |
| identification probe per unit | 300 ms | `vag-cli-core/src/units.rs:121-130` |
| the client's own default | 2 s | `vag-uds-client/src/pdu.rs:12-14` |
| board: first answer | 500 ms | `vag-dash-fw/src/exchange.rs:39`, fw `dash.rs:1540-1545` |
| board: after `78` | 5 s each, 10 s in all | fw `dash.rs:1579-1585` |
| board: fault-count exchange | 2 s from its start | fw `faults.rs:59` |

`78` is handled as the standard says: a fresh P2* after each pending answer
(`bus/task.rs:313-321`), and a late answer to an earlier request is discarded by
`schedule::answers` (`schedule/mod.rs:276-313`) on both sides.

**How it breaks.** **Probably no.** Every wait is a multiple of the defaults a unit has to meet
unless its ODX says otherwise. The tightest is the 300 ms identification probe, and a unit that
misses it is dropped from `watch`/`measure` for the run without a word (`units.rs:127-130`). A
gateway that takes longer to route the first request to a sleeping sub-bus would do that.
**Inferred** risk, not observed.

**The fix.** If a newer car shows it: P2/P2* per unit from the project's communication
parameters (whether the ODIS project carries them is `labels.md`'s question), and one retry of
the probe at P2* before a listed unit is dropped. S.

## 6. Identity — `F187`, `F19E`, `F1A2`, the part check, variant selection

**What the code assumes.**

- A unit says what it is through `F187` (part number), `F197` (component), `F19E` (ODX file
  name), `F1A2` (ODX version) — `units.rs:121-146`, `survey.rs:65-75`. The first three are
  ISO 14229-1 identification identifiers; `F1A2` is in the manufacturer range.
- A unit that does not answer `F187` is absent — except the engine (`units.rs:127-130`).
- Its variant is `<F19E>_<first three digits of F1A2>`; without three leading digits only a
  `Family` match is possible (`vag-data-labels/src/label_files.rs:129-218`, used by
  `vag-cli-core/src/extracted.rs:344-370`), and a `Family` match is used when it is the best
  there is (`extracted.rs:352-369`).
- The board checks each plan unit's `F187` against the number the plan was built with
  (`vag-dash-render/src/plan.rs:187-228`) — the car's own answer, nothing assumed.
- The VIN is read from the engine only (`units.rs:58-85`), and every per-car file is named after
  it (`units.rs:60-66`). `vagcan info` reads units `01` and `02` and nothing else
  (`vag-cli/src/main.rs:897-911`).
- The label project is **not** chosen by the car: `project::covering()` always answers `None`
  (`vag-cli-core/src/project.rs:200-234,257-274`), so the project is the flag, the environment,
  `config.json`, or the only one on disk.

**How it breaks on a newer car.**

- **No: the identifiers (F1).** VCDS prints an "ASAM Dataset" line of an ODX name and a
  six-digit version, beside the `.rod` file it opened; that is the `F19E`/`F1A2` join
  `label_files.rs:143-153` makes (the mapping is **inferred** — no reference-car Auto-Scan is in
  the repo to show it). 2020+ units answer in the same form: the Golf 8's engine
  `EV_ECM15TFS01105E906012N 002002`, its gateway
  `EV_GatewMQB2020 004008`; the ID.3's vehicle control unit `EV_VCU00XXX0200EA906012AA 001009`
  with file `EV_VCU00XXX0200EA906012AA.rod`, its antenna `EV_TrxMLGEMEB 006014` with
  `EV_TrxMLGEMEB_006.rod` — exactly the `Exact` and `Version` shapes the rule ranks (§13).
- **Maybe, silently: a `Family` match (F2).** The ID.3's gateway answers
  `EV_GatewICAS1MEB 005001`, and the file VCDS uses is `EV_GatewICAS1MEB_VWE3.rod` — to
  `vagcan`'s rule a `Family` match only. Here that is the right file; elsewhere a `Family` match
  reads the alphabetically first variants of a family (`extracted.rs:325-329,356-369`) —
  plausible identifiers and scalings of another software version. Nothing on screen says the
  variant was not confirmed.
- **Maybe: VIN and `info` (F3).** The ID.3 has an address `01`, a vehicle control unit
  (`J623`, component `E-Vehicle`), and no `02` (§13). If that unit sits on `0x7E0` and answers
  `F190`, the VIN read works; `info` then shows one of its two units. If not, there is no VIN, so
  no car file, no saved sessions, no survey cache — and `dev dash build` finds no survey
  (`vag-cli-core/src/dash.rs:2427`).
- **Maybe: the project (F4).** With more than one project on disk, a newer car is read against
  whichever was set up last; its units' `F19E` then match nothing, so it gets raw bytes, not
  wrong numbers.

**The fix.**

- Say when a unit's rows came from a `Family` match, per unit, in `watch`/`measure`/`dash` and in
  `survey` output. S, `vag-cli-core/src/extracted.rs`.
- VIN from the engine, then the gateway, then the first listed unit that answers `F190`. S.
- `info` from the walk (§2): the units that answer, not `01`/`02`. S.
- Fill `project::covering()` — the seam is placed and ordered; it needs the project's own
  coverage list (`labels.md` §2.4). M, mostly in `vag-data-labels`.

**Not known.** Which CAN id the ID.3's `01` answers on. Whether a VAG gateway answers `F190`
(**inferred** yes; one read).

## 7. The slcan backend — bitrate, frame shape

**What the code assumes.**

- 500 kbit/s, always: `device::open` passes `SlcanBitrate::Rate500k` for every command
  (`vag-cli-core/src/device.rs:552-567`) — "VW's diagnostic CAN is 500 kbit/s (ISO 15765-4)".
  The board's `slcan` image starts at 500 kbit/s too (`vag-dash-fw/src/slcan.rs:622-632`) and
  takes `Sn` (`slcan.rs:775`).
- The channel opens in `Normal`: it acknowledges and may transmit from the first moment
  (`device.rs:577-581`).
- Classic frames: `encode_frame` refuses more than 8 bytes, `decode_frame` a DLC over 8
  (`vag-uds-can/src/slcan.rs:44-91`); the backend trait is "one classic CAN frame"
  (`vag-uds-can/src/backend.rs:32-43`). Both id kinds pass (`t`/`T`) — no 11-bit assumption here.

**How it breaks.** **No** on classic CAN: ISO 15765-4 allows 250 and 500 kbit/s, VAG passenger
cars use 500 (the reference car; **inferred** for the rest — the ID.3 answers a classic-CAN
HEX-V2, §1), and a 29-bit frame passes this layer unchanged. **Yes, if** the OBD CAN is FD:
Lawicel slcan defines no FD frame, and whether the adapters in use carry one is hardware,
`transport.md` §3.

**The fix.** Nothing for classic CAN. For FD: an FD-capable adapter and backend (a frame-size
query on `CanBackend`, an slcan-FD or gs_usb driver) — M plus a purchase; `transport.md`
decides whether it is needed. Either way, open listen-only first and go `Normal` only on a bus
that reads cleanly (the same as §8) — cheap insurance against a wrong rate. S.

## 8. The firmware — TWAI setup

**What the code assumes.**

- 500 kbit/s, `TwaiMode::Normal` — transmits and acknowledges from power-on
  (`vag-dash-fw/src/bin/dash.rs:1866-1887`).
- One 11-bit acceptance filter, from the plan's answer ids, moved per exchange
  (`dash.rs:1622-1674,1893,1919-1921`, `vag-uds-can/src/filter.rs`).
- Every exchange addresses `CanId::Standard` (`dash.rs:1707-1712`); the planner's `Unit` is
  `u16` and the host link 11-bit (§1).
- The TWAI backend itself converts both id kinds and refuses more than 8 data bytes
  (`vag-dash-fw/src/can.rs:157-207`).

**How it breaks.**

- **Yes, if** 29-bit: §1 — silence from every unit, the badge `?`.
- **Yes, and worse, if** the OBD CAN is FD. The ESP32-C3's TWAI is a classic controller —
  esp-hal's API has no FD frame (`transport.md` §3 owns the chip fact). In `Normal` a classic
  controller does not just miss FD frames: it answers them with error frames, disturbing every
  FD node on the bus (**inferred** from how ISO 11898-1:2015 treats classic nodes on an FD bus).
  A board left like that is what `CLAUDE.md`'s safety section guards against — a board that
  provokes the car on its own.

**The fix.**

- 29-bit: §1 steps 2, 4, 5 — plan ids as `CanId`, the extended filter, link v2. S–M.
- FD: never come up in `Normal` on a bus the board cannot read. Start `ListenOnly` — the default
  `can.rs:18-25` already argues for on a car — watch the error counters for a moment, switch to
  `Normal` only on a clean bus, else stay listen-only and say "this car's diagnostic CAN is not
  classic CAN" on the panel. S, `vag-dash-fw`; bench-testable against an FD node, one FD car to
  confirm. No RAM worth measuring.

## 9. The scheduler — one exchange in flight, the rates

**What the code assumes.** One exchange in flight (`schedule/mod.rs:69-80`, `bus/mod.rs:9-19`);
100 exchanges a second, a foreground floor of 25, 8 identifiers a request, backoff
250 ms → 2 s, starvation after 5 s (`schedule/mod.rs:133-194`); the fastest subscription 20 ms
(`guard.rs:127-128`).

**How it breaks.** **No.** One exchange at a time is correct on any CAN car — a throughput choice,
not an assumption about the car; the rates are the owner's budget. `max_dids_per_request = 8` is
the one number measured on the reference car (`schedule/mod.rs:142-145`), and the planner learns
a unit that takes fewer — single-only after NRC `13`/`14`, an empty answer or three unsplittable
ones (`schedule/mod.rs:30-35`) — so a newer unit that differs costs a few requests, not a wrong
value. The only car-shaped thing here is the `u16` in `Unit` (A1).

**Not known.** Whether a newer gateway rate-limits diagnostic requests (nothing found).

## 10. The moving-car check — which DID gives road speed

**What the code assumes.** Road speed is `22 F40D` asked of `0x7E0`, answered on `0x7E8` — SAE
J1979 PID `0D` at its UDS mirror, one byte of km/h:

- on the board, radio and cable alike (`vag-uds-client/src/guard.rs:10-15,141-153`, decoder
  `guard.rs:294-302`, the exchange `remote.rs:119-123,413,507`);
- on the laptop, before `faults --extended`, `survey --extended`, every `survey` without
  `--while-driving`, and `units --identify` without it (`vag-cli-diag/src/safety.rs:17-58`;
  callers `faults.rs:429-435`, `survey.rs:409-432`, `vag-cli/src/main.rs:1122-1142`).

"No answer" is "moving" in both, as `CLAUDE.md` requires (`guard.rs:432-433`, `safety.rs:51-56`).

**How it breaks on a newer car.**

1. **Maybe (J1): a BEV's `0x7E0`.** The ID.3's address `01` is a vehicle control unit
   (`J623-EBJC`, `E-Vehicle`, `EV_VCU…`, §13), not an engine. Whether it sits on `0x7E0` and
   serves J1979's `F40D` is not known: OBD mode 01 is a legal duty of combustion-engine cars, and
   what a BEV's unit keeps of it is up to VW. If it does not answer, the check never passes —
   every session change refused on board and laptop, which is fail-safe by design.
2. **Maybe, unsafe (J2): a unit at `0x7E0` whose `F40D` is not J1979's.** The code's own
   evidence says VW units answer `F40D` in their own layouts: the reference gearbox gives two
   little-endian bytes ×0.01 (`vag-cli-core/src/plan.rs:235-238`;
   `research/vcds-registry/README.md` §1), the climate unit's `F4xx` are not the standard's
   (`address.rs:86-92`). The board's decoder insists on exactly one data byte (`guard.rs:297-301`)
   and fails safe. **The laptop's does not**: `road_speed_kmh` takes the first byte of whatever
   came back (`safety.rs:29-31`; `pdu::parse_rdbi_response` checks only the echo,
   `pdu.rs:90-100`). A two-byte ×0.01 answer at 25.6 km/h is `00 0A` — read as **0 km/h,
   stationary**. Latent on the reference car, whose `0x7E0` is a J1979 engine; a wrong
   "stationary" on any car whose `0x7E0` is something else.
3. **Yes wherever 1 happens (J3).** A *parked* `survey` and a parked `units --identify` are
   refused too, and the text offers `--while-driving` as the way out (`survey.rs:421-431`,
   `main.rs:1134-1140`). On a car where the check can never pass, the only survey left is the
   one flagged as dangerous.
4. *If 29-bit*, the check addresses `0x7E0` by construction; whether such a car serves J1979
   there is `transport.md`'s question.

**The fix.**

- **Now, whatever else is decided (J2):** the laptop decodes with `guard::road_speed` — exactly
  one byte — instead of keeping a second decoder. S, no car.
- **Resolve the speed source from data (J1), `0x7E0`/`F40D` first.** Vehicle speed is one
  VW-wide measurement id in the label data — `IDE00075`, on 883 rows and 488 variants of the
  owner's ODIS cache: 381 engine, 17 body-module, 10 gearbox, 9 brake and 8 ACC variants among
  them (`sqlite3 ~/.vagcan/data/SK37X/cache.sqlite`, 2026-09-28); VCDS carries the same through `RM.rod`
  (`research/vcds-registry/README.md` §1). `measure` already resolves road speed by role, finest
  first (`vag-cli-measure/src/channels.rs:189-198,395-403`). The laptop resolves one speed
  source per car the same way (unit, DID, bit layout, factor) and records it with the VIN; the
  dash plan carries it (`vag-dash-render/src/plan.rs`, a `speed_check` beside `stopwatch`); the
  board's guard asks it instead of the hard-coded request (`guard.rs:141-146`) and falls back to
  `0x7E0`/`F40D` when the plan has none. The rule does not change: no answer, a refusal, or
  anything but exactly the resolved layout is "moving". M — `vag-uds-client` guard,
  `vag-dash-render` plan, `vag-cli-core` generator; RAM a few bytes.
- **The refusal (J3) names what it tried** and, when no source exists, says the check cannot run
  on this car — without pointing at `--while-driving`. S.

Needs a car: yes — one BEV, to see what answers at `0x7E0`, and that the resolved source reads 0
at a standstill and moves with the car.

**Not known.** What answers at `0x7E0` on MEB / PPE; whether a newer ICE car's `0x7E0` still
answers `F40D` as one byte (**inferred** yes — J1979 binds it).

## 11. The dash plan and the measure roles

**The dash plan** (`vag-dash-render/src/plan.rs`) is built on the laptop from the car's answers
and the label data and names no car: units by request and response id and the `F187` they
answered (`plan.rs:178-190`), channels by unit, DID, bit layout and scaling (`plan.rs:234-260`),
the stopwatch's `km_h_per_unit` measured by the owner (`plan.rs:133-142`,
`vag-cli-core/src/dash.rs:71,728-745`). It inherits the 11-bit ids and the answer id taken from
`UnitAddress::from_request` at build time (`vag-cli-core/src/dash.rs:1570`, `NoResponseRule`) —
A1/A2 — and the survey cache filed under the VIN (§6, F3).

**The measure roles** (`vag-cli-measure/src/channels.rs:189-300`) find channels by English words
in a row's name: speed by "vehicle speed"/"road speed", and three more **required** roles —
"engine speed", "selected gear", "accelerator pedal position" (`channels.rs:199-225`). A
required role that matches nothing refuses the run (`channels.rs:439-465`). `watch`'s default
selection works the same way (`vag-cli-core/src/plan.rs:501-536`), and so does the
actual/specified pairing (`plan.rs:142-166`).

**How it breaks on a newer car.**

- **Yes, on a BEV: `measure` refuses (K1).** No engine speed and no selected gear to match
  (**inferred** from the drivetrain; the MEB/PPE label families are in the corpus, `labels.md`
  §1.2, so the row names can be checked offline once readable).
- **Maybe: a project whose names are not English.** The roles match `def.name`, which for an ODIS
  row is the project's own name (`vag-cli-core/src/extracted.rs:381-390`). The owner's project is
  English ("Vehicle speed", "Vehicle Speed Sensor"); whether projects come in other languages is
  `labels.md`'s.
- The OBD-II set is offered on `0x7E0` whatever the car (`plan.rs:195-222`) — on a BEV, channels
  that may never answer. Harmless.

**The fix.**

- **Roles keyed by VW's measurement ids, words as the fallback.** Every ODIS row carries a text id
  shared across variants and languages — `IDE00075` vehicle speed (883 rows), `IDE00090` selected
  gear (62), `IDE00086` accelerator pedal position D (819), same query — and `dash.toml` already
  names channels that way (`"01:IDE00190"`, `vag-cli-core/src/dash.rs:3710`). They are VW's data
  dictionary, like a J1979 PID number, not one car's identifiers; still numbers in source, so one
  table with the reason beside it, and the words stay for a VCDS-only project. S–M,
  `vag-cli-measure/src/channels.rs`, `vag-cli-core/src/plan.rs`.
- **Required roles by drivetrain.** Speed and pedal everywhere; engine speed and gear only where
  the car has them — a car with no row for either is a car without them, not a failed
  resolution. S.

Needs a car: one BEV run of `measure` after the change.

## 12. Magic numbers from this one car

What `CLAUDE.md` forbids — one car's data in the path other cars take. Covered above:
`BUILT_IN_SHORT_NUMBERS` (§1, A7), `0x710`/`2A26` and the `0x76`/`0x77` case (§2),
`SURVEY_RANGES` (§3, C2), `max_dids_per_request` (§9, adaptive). The rest of the non-test code
was swept for 3- and 4-digit hex literals: what remains is ISO identification identifiers
(`F18x`–`F19x`), VW's `F1A2` and `0600`, the J1979 mirror `F40D`, adapter USB ids — and these:

**The fault-record layout (L1).** `FaultContext` reads extended-data record `0x01` as
`priority, occurrences, counter u16, mileage u24, 2 bytes, clock u32`, the layout "every unit on
the reference car answers" (`vag-uds-client/src/dtc.rs:44-90`), with the packed date established
against two VCDS printouts of this car (`dtc.rs:92-155`).

- `FaultContext::parse` accepts any record of 13 bytes or more (`dtc.rs:188-199`), and `faults`
  applies it to **every** record `19 06 … FF` returns, whatever its number
  (`vag-cli-diag/src/faults.rs:589-604`). A unit whose record `0x01` is laid out otherwise, or that
  returns a second record of 13+ bytes, gets a confident wrong mileage and date — the failure
  `dtc.rs:210-216` describes for `02BD` on the door units, which `UnitStamp` guards against by
  insisting on its length (`dtc.rs:227-240`). **Maybe** on a newer car (**inferred**: VW's
  environment data may well be unchanged; nothing checks that it is).
- Fix: parse record `0x01` only, and only at exactly 13 bytes — the length it was established
  at; take the layout from the unit's ODX environment-data description where the project has
  one (`labels.md`). S, `dtc.rs` + `faults.rs`. A newer unit with a stored code confirms it.

**`UnitStamp` at `02BD`** (`dtc.rs:202-241`): same provenance, but it refuses any length but 10 —
fails safe. **No.**

## 13. Sources, and what is not known

**Sourced facts about newer cars** — VCDS Auto-Scans posted on Ross-Tech's own forum, and
Ross-Tech's release notes:

- 2020 VW ID.3 (E11), full Auto-Scan: units `01` `J623-EBJC` `E-Vehicle`, `19` `J533`
  `ICAS1 Host-SG`, `51` `J841` electric drive, `8C` `J840` battery, no `02`, and ten four-digit
  addresses `8105`–`813F` — <https://forums.ross-tech.com/index.php?threads/24641/>
- "VCDS HEX V2 and VW ID.3": an ID.3 scanned with "VCDS Version: 20.9.1.0 HEX-V2"; ASAM datasets
  `EV_VCU00XXX0200EA906012AA 001009`, `EV_GatewICAS1MEB 005001` → `EV_GatewICAS1MEB_VWE3.rod`,
  `EV_TrxMLGEMEB 006014` → `EV_TrxMLGEMEB_006.rod` —
  <https://forums.ross-tech.com/index.php?threads/25398/>
- Golf 8 (`CD-VW38`), VCDS 23.11 on a HEX-NET2: scan list
  `01 03 05 08 09 10 13 15 17 19 23 2B 3C 42 44 52 5F 75 A5 D6 D7 8107 C002 C003`, gateway
  `EV_GatewMQB2020 004008` — <https://forums.ross-tech.com/index.php?threads/40985/>
- VCDS revision history: 20.4 "Support for DoIP protocol (HN2 interfaces only)"; 20.12
  "Preliminary support for Mk.8 and MEB (ID.x) cars"; 21.9 "SFD Support … added" —
  <https://www.ross-tech.com/vcds/revisions.php>
- esp-hal `1.0.0-rc.0`, `src/twai/filter.rs:95-99,224-230` (the crate the firmware pins), local
  cargo registry.

The standards named (ISO 14229-1/-2, ISO 15765-2:2016, ISO 15765-4, ISO 11898-1, SAE J1979) are
cited as the code cites them; none was re-read for this note.

**Open questions, and the cheapest read that settles each.** None needs a write service.

| question | settles | how |
|---|---|---|
| Which CAN ids do the four-digit units (`8107`, `81xx`, `C002`) answer on — 29-bit? | A1–A6, B3 | `transport.md` §4; `dev sniff` beside VCDS on a Golf 8 or ID.3 |
| How does a `GW2020` / `ICAS1` gateway list its units; does it answer `2A26`? | B1–B3 | one `vagcan units`; the gateway's declared identifiers |
| Do bits `0x76`/`0x77` on the reference car answer at `0x17FC0076`/`…77`? | B4 | one `22 F187` each, once §1 can address them |
| Is any 2020+ VAG OBD-port CAN FD? | D1, G1, H1 | `transport.md` §3; a listen-only `dev sniff` |
| What answers at `0x7E0` on a BEV, and is its `F40D` one byte? | F3, J1, J2 | one read on an MEB car |
| Does a VAG gateway answer `F190`? | F3 | one read |
| Unit counts on MLB-evo / PPE flagships | C1 | one `vagcan units` |
| Is a newer unit's fault record `0x01` the reference layout? | L1 | `faults --details` on one newer unit with a stored code |
