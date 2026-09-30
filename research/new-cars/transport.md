# Newer VAG cars — protocol, transport and hardware

Status: **complete** (2026-09-28). Research only — no code, no car, no adapter, no BLE. Written in
two sessions: a first draft (`ed680b1`), then a review that checked it against its sources, filled
the rest, and corrected seven statements (listed before the sources).

Question (owner, 2026-09-28): what stops `vagcan` reading cars from about 2020 on
(MQB-evo, MEB, MLB-evo/PPE), and what the fix is. This file is the protocol / transport /
hardware part. Label data is [`labels.md`](labels.md); hard-coded assumptions in the code are
[`code.md`](code.md).

Every claim is marked **sourced** (with URL), **measured here**, or **inferred**.

**In short.** Nothing at this layer stops a 2020+ VAG car from being read over its diagnostic CAN
at 500 kbit/s with classic frames: the CAN stays at the socket beside DoIP, and a CAN-only HEX-V2
and a classic ESP32 both read an ID.3. What stops `vagcan` is **addressing**: on the reference
car's own platform 13 of 54 units are already out of its reach — 10 on 29-bit ids, 2 behind an
extra address byte, 1 outside both 11-bit rules — and on an ID.3 the VIN and the odometer sit behind
29-bit ids. The car's ODIS project holds every unit's ids and timings, ready to be read at `setup`.
SFD guards writes, not reads; the gateway's diagnostic filter may hide units, and its status is a
plain read. CAN FD is, for now, a safety rule for the board — come up listen-only — not a feature to
build.

## Summary

| blocker | which cars | blocks reading? | confidence | solution | cost | needs a car? |
|---|---|---|---|---|---|---|
| SFD / SFD2 (§1) | VAG brands from MY 2020/21, every market (SFD); most MY 2024+ (SFD2) | **no** — guards write services; `vagcan` sends none | high (VW SSP 706, Ross-Tech, VW's terms) | nothing to build; no unlock route exists for a private tool and none is proposed | — | no |
| Gateway diagnostic filter (§1, §5) | "newer models" (Ross-Tech); the firewall before it since MLBevo (A8 4N, 2017) | **partly** — the firewall passes reads (Audi's white list); the filter "may" block some units entirely | medium | read the gateway's filter status (a `22` read, identifier from label data) and say "blocked by the filter", not "absent"; whether to also suggest opening the bonnet — the car's own off switch, which VW's tester prompts — is the owner's call (§1) | S | yes — one 2021+ car with the filter active |
| DoIP on the OBD port (§2) | MLBevo (A8 4N 2017, wires since Q7 4M), Golf 8 "Gateway high", MEB, e-tron GT | **no** — added beside the diagnostic CAN; a CAN-only HEX-V2 reads every unit of a 2020 ID.3 | medium-high (sourced; no DoIP-only unit known) | keep CAN for reading; DoIP only for speed later: a laptop `UnitLink` over ISO 13400-2 plus an OBD-to-RJ45 cable with the activation line; not on the C3 | M + cable, optional | only to confirm the optional link |
| CAN FD on the diagnostic CAN (§3) | MEB (ID.3, ID.4: 500 k / 2 M), A3 8Y; A8 4N and e-tron GT are 500 k classic; the Golf 8 is drawn 500 k without the FD mark | **no, so far** — a classic ESP32 TWAI + SN65HVD230 reads an ID.3 with classic frames | medium (secondary for MEB, none for A3 8Y) | board: come up listen-only, go `Normal` only on a clean bus — judged by the bus-error interrupt, since listen-only freezes the error counters — never error-frame FD; laptop: surface `d`/`D`/`b`/`B` lines instead of dropping them; FD ISO-TP only if a car answers only in FD (the CANable 2.0 Pro can) | S; M for FD reads; L for an FD board | yes — one FD car; the listen-only start is bench-testable |
| 29-bit CAN ids (§4) | **already MQB**: 10 of 54 units in the reference car's own ODIS project; MEB: units at logical `0x76` (VIN, odometer), `0x7B`, `0xB9` | **yes** for those units — `vagcan` cannot address them | high (measured in the project; three secondary MEB sources agree) | request `0x17FC0000+la`, answer `0x17FE0000+la`, normal addressing — taken per unit from the project's `CP_CanPhysReqId`/`CP_CanRespUSDTId`, not from rules; the code side is `code.md` §1 | M (`code.md` A1–A6) + S–M (read the parameters at `setup`) | yes — one read at a 29-bit unit |
| ISO-TP extended addressing (§4) | reference platform: sunroof and battery monitoring on `0x728`, LED headlight power modules on 29-bit | **yes** for those units | high (measured in the project) | an addressing-format parameter in `IsoTpCan`: N_TA byte first, 6-byte SF/CF payloads; format and N_TA from the project | S | yes — one such unit |
| Per-unit P2 / P2\* (§5) | any; reference project: rear door units P2 1.9 s, telematics P2\* up to 600 s | **partly** — a slow unit misses the 300 ms identification probe and is dropped without a word | medium (values measured in the project; car behaviour not seen) | timing per unit from the project (`CP_P2Max`, `CP_P2Star`), VW's 200 ms / 5.15 s as the default; retry the probe once with the unit's own P2 | S | yes, to observe |
| Bus speed (§5) | every car found: 500 kbit/s arbitration; FD data phase 2 Mbit/s where FD | **no** | high | keep 500 k; open listen-only first (cheap insurance) | — | no |
| ISO-TP length and flow control (§5) | FD cars (64-byte frames); answers over 4095 bytes; units sending more than 8 `FC.WAIT` | **no** on classic CAN (**inferred**) | medium | first-frame escape (`code.md` §4); N_WFTmax 15 as VW's protocol layer has it | S | no |

## What vagcan does today (the baseline)

All **measured here** — read off the tree at `db40ed6`, 2026-09-28.

| layer | what the code does | where |
|---|---|---|
| physical | classic CAN (ISO 11898-1 CAN 2.0), 500 kbit/s fixed on both transports | laptop: `vag-cli-core/src/device.rs:564` opens slcan with `SlcanBitrate::Rate500k`; board: `vag-dash-fw/src/bin/dash.rs:1882` `BaudRate::B500K` |
| frame | ≤ 8 data bytes; `CanFrame::new` panics past 8, slcan `encode_frame` refuses past 8 | `vag-uds-transport/src/frame.rs`, `vag-uds-can/src/slcan.rs:45` |
| id width | raw `u32` with a SocketCAN-style `CAN_EFF_FLAG`; slcan encodes/decodes `T` (29-bit) lines; `IsoTpCan::new` takes an extended id (test `new_accepts_extended_ids`) | `vag-uds-can/src/backend.rs`, `slcan.rs:50,78`, `isotp.rs:460` |
| addressing | `UnitAddress { request: u16, response: u16 }` knows two 11-bit rules only: `0x7E0..0x7E7 → +8` (ISO 15765-4) and `0x700..0x795 → +0x6A` (VW); every command builds `CanId::Standard` | `vag-uds-client/src/address.rs:34-72`, e.g. `vag-cli-diag/src/faults.rs:491` |
| ISO-TP | normal addressing only (no N_TA byte), frames padded to 8 with `0x00`, 12-bit first-frame length (≤ 4095 bytes), no escape first frame, no CAN FD data lengths; N_Bs 1000 ms, N_WFTmax 8 | `vag-uds-can/src/isotp.rs` |
| board filter | one 11-bit code/mask acceptance filter (`SingleStandardFilter`), moved per exchange | `vag-uds-can/src/filter.rs`, `vag-dash-fw/src/bin/dash.rs:1632` |
| UDS timing | a scheduled read waits 500 ms (10 × a P2 of 50 ms; *the draft's "10 × P2 = 50 ms" reworded on review*), `7F xx 78` extends by 5 s (ISO 14229-2 default P2\*), at most 30 pendings; a raw exchange waits 2 s | `vag-cli-core/src/bus/mod.rs:60-72,396`, `vag-uds-client/src/uds_async.rs` |
| sessions | reads run in the **default** session; `10 03` (extended) is opt-in and refused on a moving car; the board refuses `10 02` | `vag-cli-diag/src/safety.rs`, `ARCHITECTURE.md` "The USB cable" |
| whole-car walk | gateway `0x710 → 0x77A`, installation list `22 2A26`; the laptop turns every bit into request `0x700 + n`, the board's fault count decodes only the VW block's 24 bytes (`0x700..0x7BF`) *(corrected on review; the draft said "VW block bits only")* | `vag-uds-client/src/gateway.rs:71-100` |
| board hardware | ESP32-C3 TWAI + SN65HVD230 (3.3 V classic-CAN transceiver), OBD pins 6/14 | `research/dash/can-bring-up.md` §4.3, §5 |

So: the frame layer already carries 29-bit ids end to end on the laptop; the address layer
and the board's filter do not. Nothing anywhere carries a CAN FD frame, and there is no
transport that is not CAN.

## 1. SFD

### Cause

**What SFD is — Ross-Tech's own page** (**sourced**: Ross-Tech wiki, "SFD", revision 9826,
<https://wiki.ross-tech.com/wiki/index.php?title=SFD&oldid=9826>, read 2026-09-28):

- *Schutz der Fahrzeugdiagnose* (SFD, "Protection of Vehicle Diagnostics") **replaces the
  old login / security-access protection**. Ross-Tech states it does not interfere with
  reading control-module identification, event and fault memory, or measuring values.
- What it may lock, and what then needs an SFD unlock: **coding, adaptation, basic
  settings, output tests**.
- Which cars: Audi, Bentley, Bugatti, CUPRA, MAN, SEAT, Škoda, Volkswagen and VW
  Commercial, **from MY 2020/2021**, the date differing by model, **in every market**. At
  first only newly introduced models and units, so one car can carry units with the old
  login and units with SFD side by side. There is no build code that says a car has SFD;
  VCDS marks each unit `SFD` in its Auto-Scan (`VCID: … SFD`).
- **SFD2** is an extension, not a replacement: signed messages for coding, adaptation and
  updates, driven by UNECE R155/R156, rolled out outside UNECE markets too. Ross-Tech's
  MEB gateway example is `EV_GatewICAS1MEBUNECE`, marked `SFD+SFD2`. PR codes `NI1/NI7/NI8/NI9`
  mean some or all units are SFD2-locked.
- Neither VCDS nor ODIS can switch SFD or SFD2 off permanently.

**The gateway's diagnostic firewall and diagnostic filter are a separate thing, and the one
that can matter to reading** (same page, section "Diagnostic Firewall & Diagnostic Filter"):

- A diagnostic **firewall** has existed since before SFD; Ross-Tech advises opening the hood
  on every MY 2015+ car before diagnostic work so that it is deactivated.
- **Newer models add a diagnostic filter** that, in Ross-Tech's words, may block "diagnostic
  access to control modules entirely". VCDS "uses a workaround" to reach them and then "may be in
  Restricted (read-only) mode", marked `-R` in the Auto-Scan (`VCID: …-R`, and `-R SFD+SFD2`
  combined); the page's examples are a seat-memory unit (address `06`) and an HVAC unit (`08`).
  Ross-Tech does not say what the workaround is.
  *(Corrected on review: the first draft said VCDS reaches them "only" in restricted mode and
  that Ross-Tech does not say how; the page says "may be", and names a workaround without
  describing it.)*
- The gateway (address `19`) reports the filter in measuring values: `IDE13754` "Diagnostic
  filter: status" with *Reason for deactivation: hood open* and *Filter is not active*, or
  *Function active: SFD protected* and *Filter active*. It is switched off temporarily by an
  SFD unlock of the gateway followed by an adaptation (`IDE16611`, "temporary deactivation"),
  which then counts down 20 km.

**Ross-Tech's current release notes say the same for SFD2** (**sourced**: "VCDS: Current Version",
Release 26.9, <https://www.ross-tech.com/vcds/download/current.php>, read 2026-09-28): most MY 2024+
cars have some functions locked with SFD2, which VCDS does not yet support; "Functions that read
data should work fine, but SFD2 will restrict changes in some control modules." The same page:
VCDS now supports on-line SFD (beta, separate subscription); "Support for some model-year 2024 and
newer cars is limited"; and "none of our legacy interfaces such as the HEX+CAN and Micro-CAN will
work properly with 2019 or newer model cars" — a transport-level change Ross-Tech does not explain
(see §5.1). Its store page adds that HEX-V2 is "Not compatible with Lamborghini, Routan, Transporter
T7 (2025+), or Amarok Mk2 (2023+)" (<https://store.ross-tech.com/shop/vchv2_ent/>).

**How the unlock works, and who may use it — VW's own terms** (**sourced**: Volkswagen AG,
"Schutz der Fahrzeugdiagnose (SFD) — Nutzungsbedingungen", *Stand: Juli 2021*,
<https://privacy.volkswagen.com/download/get-document-content/57b3edcf-a670-4cfc-87b5-f6afd70f3bb2>,
PDF, read 2026-09-28):

- The tester asks VW's **SFD backend** for an **SFD token**; the backend creates and **signs**
  it for an authorised user; the tester passes the token to the control unit, which checks it
  and then grants access (§2.2). Every access is logged against the person, the vehicle and
  the time (preamble, §2.2).
- Access needs a **personal account** — for ODIS Service through VW's dealer portal / Group
  Retail Portal (§1.2.2); for a third-party tester, through that tester's vendor, and only
  vendors "that have an agreement with the Volkswagen Group" qualify (§1.1, §1.2.3).
- Tokens may be fetched only for vehicles entrusted to the user's organisation for diagnosis;
  any other use, **explicitly including private use**, is not permitted (§2.3, §2.4). Getting
  round the security measures is forbidden (§3.4).

**VW's own statement of scope: SFD protects write services** (**sourced**: VW self-study
programme SSP 706 *The Golf 2020*, technical status 02/2020, section "The vehicle diagnostic
protection", copy at <https://esperformance.net/ssp/vw/SSP_706_EN.pdf>, read 2026-09-28):

- "Vehicle diagnostic protection (SFD) refers to the protection of diagnostic **write services**
  from unauthorised access." The example given is adaptation values that can be changed only
  after the unit has granted access.
- Before SFD a unit was opened with a fixed per-unit PIN from which the tester computed an access
  code (the "login" of older cars). With SFD the access code is generated online by VW's SFD back
  end, valid for one unit and one VIN, requested by Guided Fault Finding; the mechanic needs "SFD
  rights in the Dealer Portal". An unlocked unit stays open 90 minutes of awake time.
- The Golf 2020's SFD units, by diagnostic address: `0003` brake electronics, `0009` central
  electrics, `0015` airbag, `0017` instrument cluster, `0019` gateway, `0023` brake servo, `004B`
  multifunction module, `005F` MMI, `0075` emergency call, `8107` aerial module. The rest keep the
  PIN method.

**The gateway's diagnostic firewall — Audi's own description, with its white list**
(**sourced**: Audi of America, *The 2019 Audi A8 Electrics and Electronics*, eSelf-Study Program
970293, p. 40, hosted by NHTSA at <https://static.nhtsa.gov/odi/tsbs/2018/MC-10144683-9999.pdf>;
the German original is SSP 664 *Audi A8 (Typ 4N) Elektrik und Elektronik*, pp. 43–44, copy at
<https://pdfcoffee.com/ssp664-wg-de-pdf-free.html>; both read 2026-09-28):

- The diagnostic firewall first appeared with the A8 4N (MLBevo, 2017) and lives in the gateway
  J533, which holds a **"White List"** of what passes while it is active. "As a basic
  principle, all read services are enabled and all write services are disabled." It arms for
  the first time once the car has covered more than 200 km, and "will be used in other Audi
  models at a future date".
- White list: accessing the diagnostics, read event memory, read measured data, read
  identification data, clear event memory, transport mode on/off, and **all services in the
  gateway**.
- Disabled: activate control elements (output tests), basic setting, replace control module,
  check SVM control module configuration.
- Switched off by opening the front lid (back on after 20 km with it closed), by losing the body
  control module J519, or by a crash signal from the airbag unit J234. The tester warns at
  diagnostic entry; the state is a measuring value of J533.
- The e-tron GT's gateway (SSP 684, 2021, <https://static.nhtsa.gov/odi/tsbs/2021/MC-10190946-0001.pdf>,
  p. 140) still lists "Manages diagnostics firewall" among its tasks.

The white list is written in ODIS function names, not UDS services. Mapped onto what vagcan
sends (**inferred**): `22` identification and measuring values, `19` fault memory and the
default-session `10 01` / `3E` of a diagnostic entry are on the passing side; nothing vagcan
sends is on the blocked side. Whether the white list admits `10 03` to a unit behind the gateway
is not stated — see "Not known".

So the only legitimate unlock is a signed token from VW's backend, through a tester whose
vendor has an agreement with VW, used by an organisation on a car it was given to diagnose.
A private owner's tool has no route to it under these terms (**inferred** from §1.1, §2.4;
the terms are dated July 2021 and may have changed since).

### Solution

**SFD itself needs nothing from `vagcan`.** SFD and SFD2 guard write services — VW's own
definition (SSP 706: "protection of diagnostic write services"), Ross-Tech's list (coding,
adaptation, basic settings, output tests), and Ross-Tech's release note for SFD2 ("Functions that
read data should work fine"). `vagcan` sends no write service; its allowlist is `22`, `19`, `10`,
`3E`, and its reads run in the default session (baseline table). **Not a blocker for reading**
(**inferred** from the sources above; confidence high for identification, fault memory and
measuring values in the default session).

**What can still make a unit look dead on a newer car is the gateway, and the gateway says so.**
Two cheap changes, neither a new service:

1. **Read the gateway's diagnostic-filter status and report it.** It is a measuring value of the
   gateway (Ross-Tech: `IDE13754` "Diagnostic filter: status", with a reason — "hood open" — or
   "Function active: SFD protected", and "Filter active" / "Filter is not active"). `vagcan` reads
   it like any other gateway measurement: identifier and scaling from the car's label data, no
   number in the code (`CLAUDE.md`). When it says "Filter active", a unit that does not answer is
   reported as "blocked by the gateway's diagnostic filter", not as "absent". **S.** Needs one
   2021+ car with the filter active, to see what a filtered unit sends back (a negative response,
   or nothing).
2. **A refused `10 03` is an answer, not a failure.** If an SFD unit refuses the extended session
   (any negative response; `22` conditionsNotCorrect or `33` securityAccessDenied are the likely
   ones, **inferred**), say so for that unit and carry on in the default session. **S.**

**The legitimate routes, and the one that is not `vagcan`'s.**

- **Opening the bonnet** is the car's own switch. Audi built the firewall to go off when the front
  lid is opened, and the group's own VAS tester tells the mechanic to "Open the hood" when it finds
  the firewall active (SSP 970293 p. 40); Ross-Tech recommends opening it on every MY 2015+ car
  before diagnosis; on cars where the filter's reason for deactivation is "hood open", that is
  what deactivates it. `vagcan` could tell the owner to open the bonnet when the gateway reports
  the filter active. It uses the car as designed and circumvents nothing — but it does switch a
  protection off, and with it off the car accepts services the white list stops, from any tester
  (`vagcan` sends none of them). **Whether `vagcan` should suggest it is the owner's decision**; if
  not, the "blocked by the gateway's diagnostic filter" report above stands alone.
- **An SFD unlock** needs a signed token from VW's backend, fetched through a tester whose vendor
  has an agreement with VW, by an organisation, for a car it was given to diagnose — private use is
  excluded (VW terms §1.1, §2.3, §2.4). Switching the filter off also needs an adaptation
  (`IDE16611`, Ross-Tech), a write service. So there is no route for `vagcan`, by design, and
  none is proposed: getting round SFD, the firewall or the filter is forbidden by VW's terms
  (§3.4) and by this project's safety rules.

Cost: S. Needs a car: yes — one 2021+ car with an active filter.

### Not known

- **What the diagnostic filter does to a plain read**: a negative response, answers to some
  identifiers only, or silence. Ross-Tech says "may", "entirely", and "a workaround", nothing more.
  A guess, **inferred** and unverified: VCDS's "restricted (read-only) mode" is simply staying in
  the default session, which is where `vagcan` reads anyway.
- Whether the firewall's white list lets `10 03` through to a unit behind the gateway, and whether
  an SFD unit refuses `10 03` without a token. The extended session is not a write service, but no
  source says it stays open.
- Which identifier and layout `IDE13754` has on each gateway — label data, per car.
- Whether VW's SFD terms have changed since the July 2021 document.

## 2. DoIP on the OBD port

### Cause

**Ross-Tech's classic-CAN interface still claims every current VAG car** (**sourced**):

- HEX-V2 product page, <https://www.ross-tech.com/vcds/hex-v2.php> (read 2026-09-28): compatible
  with "all diagnostic-capable VW/Audi passenger cars from 1996 to current; K, K+L, dual-K, or
  CAN". Its exception list names Routan vans ("re-badged Chryslers"), **Lamborghini ("these
  require a HEX-NET")** and some 1991–94 Audi TDI engines. HEX-V2 has no DoIP (revision history
  20.4: DoIP on "HN2 interfaces only"). *(Corrected on review: the first draft said the list
  names "only Routan vans and some 1991–94 Audi TDIs" and left out Lamborghini.)*
- HEX-NET product page, <https://www.ross-tech.com/vcds/hex-net.php>: the same compatibility
  line, plus "As of late 2020, 'HN2' HEX-NETs in blue shells support car-side communications via
  DoIP".
- VCDS revision history, <https://www.ross-tech.com/vcds/revisions.php>: 20.4 (Apr 2020)
  "Support for DoIP protocol (HN2 interfaces only)"; 20.12 (Dec 2020) "Preliminary support for
  Mk.8 and MEB (ID.x) cars"; 21.3 "New Codeblock for HEX-NET/HEX-V2 … with new addresses";
  21.9 "SFD Support … (Offline Tokens only)", "DoIP Support enhanced", "Support for MEB-Platform
  Models enhanced"; 23.3 "Numerous corrections for DoIP communication".

**Audi's own account: DoIP is added beside CAN, for flashing speed** (**sourced**: SSP 664, *Audi
A8 (Typ 4N) Elektrik und Elektronik*, pp. 36, 44, <https://pdfcoffee.com/ssp664-wg-de-pdf-free.html>):

- The A8 4N bus table lists **"CAN-Diagnose … 500 kbit/s"** and **Ethernet 100 Mbit/s** "between
  the vehicle diagnostic tester and the gateway" (Fast Ethernet), used to cut the time for
  parametrising and updating control units.
- The section is titled *Diagnose über CAN und DoIP* — diagnosis over CAN **and** DoIP: the
  Ethernet link is *additional* ("mit der zusätzlichen Ethernet-Verbindung"), to use FlexRay's
  bandwidth when flashing engine and gearbox units and to flash CAN units in parallel. It needs
  the VAS 6154 interface.
- The extra wires at the diagnostic connector have been fitted **since the Audi Q7 (Typ 4M), the
  first MLBevo car** (2015); they look like FlexRay wires but carry Ethernet.
- The DoIP figure draws the diagnostic CAN as "1 MBit/s" — the ceiling of CAN, which the text
  gives as the reason for DoIP; the bus table gives its rate as 500 kbit/s.

**MQB-evo and MEB carry DoIP at the socket too, beside the diagnostic CAN** (**sourced**, VW
self-study programmes):

- Golf 8 (SSP 706, 02/2020, <https://esperformance.net/ssp/vw/SSP_706_EN.pdf>): the "Gateway
  high" Ethernet module has one of its six 100 Mbit/s ports on the diagnostic connector; the
  diagnostic CAN (500 kbit/s) stays. A later "Gateway low" has **no Ethernet module**.
- ID.3 (SSP 709, 07/2020, <https://esperformance.net/ssp/vw/SSP_709_EN.pdf>) and ID.4 (SSP 718,
  01/2021, <https://esperformance.net/ssp/vw/SSP_718_EN.pdf>): "The diagnostic CAN … and the Ethernet
  with 100 Mbit/sec are connected to the diagnostic connector (T16)"; "Diagnostic Ethernet:
  100 Mbit/s".
- Audi e-tron GT (J1, SSP 684, 2021, <https://static.nhtsa.gov/odi/tsbs/2021/MC-10190946-0001.pdf>,
  p. 140): the figure "Diagnostics CAN/Ethernet" draws the gateway J533 and the diagnostic
  connection U31; its bus table gives the diagnostics CAN 500 kbit/s and Ethernet 100 Mbit/s
  (p. 130).

So DoIP exists car-side on VAG cars since MLBevo (A8 4N, 2017; the wires since the Q7 4M,
2015), it was added beside the diagnostic CAN rather than in place of it, and Ross-Tech sells a
CAN-only interface as covering every current car, Golf 8 and ID.x included (**inferred** from
the pages together; neither source says in one sentence "CAN on pins 6/14 answers on every DoIP
car").

**A CAN-only interface reached every unit of a 2020 ID.3, the Ethernet-hosted ones included**
(**sourced**: Ross-Tech forum thread 25398, <https://forums.ross-tech.com/index.php?threads/25398/>,
read 2026-09-28). The Auto-Scan says `VCDS Version: 20.9.1.0 HEX-V2 CB: 0.4529.4` and lists 35
addresses, `01 … D7` plus `8105 8107 811E 811F 8120 8121 8123 8124 8125`; every one of them prints
its identification (part number, component, ASAM dataset). `8123`'s status line says "Cannot be
reached 0100" although its identification is printed. `8123`–`8125` are "App server 1 system 1
adaptive", "App server 1 system 2 Java" and "App server 3 system 1 infotaint." (`SWC4`–`SWC6`,
components `ICAS1 Sys_Ada`, `ICAS1 Sys_Jav`, `IC3-IVI-D-EU`) — software units on the ICAS
computers. SSP 718 (<https://esperformance.net/ssp/vw/SSP_718_EN.pdf>) says the ICAS3 is "used by
several virtual control units" (addresses `5F` and `8125`) and sits on the 1 Gbit/s Ethernet. So the gateway routes a diagnostic request from the OBD
port's CAN to units that live on Ethernet (**inferred** from the two together). The HEX-NET2 scan
of an ID.3 in thread 24641 lists the same addresses, with the cluster as `813F` where this one
has `17` (Ross-Tech moved it back to `17` in that thread).

**VW's own tester talks DoIP through its interface, and CAN FD too — local, VW-internal**
(**measured here**, read from `~/Downloads/D-PDU_API_31.0.0/…/VW_D-PDU_API_31.0.0_Releasenotes.pdf`,
VW D-PDU API 31.0.0, 17.12.2024; the document is marked internal and confidential, so it is
summarised in translation here and not copied into the repo): the VAS 6154 interface is supported
without its CAN FD function, the VAS 6154A with it; the D-PDU API tunnels DoIP traffic through the
VAS 6154; and it offers CAN FD to independent workshops' pass-thru interfaces per SAE J2534-2
`FD_CAN_PS`, with bit sample point and SJW fixed to SAE J2284's values. VW's dealer tool needed new
interface hardware for CAN FD, so some VW diagnosis at the socket uses it (**inferred**).

### Solution

**Keep CAN as the transport for reading; DoIP is not needed for it** (**inferred**, from the
sourced facts above):

- A HEX-V2 — CAN only — read every unit of an ID.3, including the Ethernet-hosted `8123`–`8125`.
- Every VW/Audi document read here keeps a diagnostic CAN at the socket beside the Ethernet pair
  (A8 4N, Golf 8, ID.3, ID.4, e-tron GT), and the Golf 8's later "Gateway low" has no Ethernet at
  all — VW has to keep full diagnosis over CAN on those cars.
- Audi describes DoIP as the fast path for parameterising and updating (SSP 664 / 970293), not as
  a replacement.

**DoIP later, only for speed, laptop only:**

- A DoIP link is one more `UnitLink` — a link that carries whole PDUs implements it directly
  (`CLAUDE.md`, "Tech stack"). ISO 13400-2 over TCP/UDP: vehicle identification, routing activation,
  diagnostic messages. The allowlist, the moving-car check and the sweep guards sit above the link
  and apply unchanged; routing activation is a DoIP message, not a UDS service.
- Cable: OBD plug to RJ45, 100BASE-TX on pins 3/11/12/13 (ISO 13400-4 option 1; option 2 uses
  1/9/12/13) and the activation line on pin 8 (**sourced**, secondary: python-doipclient's primer,
  <https://python-doipclient.readthedocs.io/en/stable/automotive_ethernet.html>). macOS needs only
  sockets.
- Not on the board: the ESP32-C3 has no Ethernet MAC — ESP-IDF's Ethernet guide for the C3 covers
  only external SPI-Ethernet modules (<https://docs.espressif.com/projects/esp-idf/en/stable/esp32c3/api-reference/network/esp_eth.html>).
  A SPI module would be a new board revision for no gain in reading.

Cost: M plus a cable, optional. Needs a car: only to confirm the optional link; the protocol can
be built against a DoIP simulator.

### Not known

- **Whether any VAG car has a unit that answers only over DoIP.** None found. Ross-Tech says
  "Support for some model-year 2024 and newer cars is limited" without a reason; PPE (Q6 e-tron,
  Macan Electric) and E³ 1.2 cars were not covered by any source read here.
- Why Lamborghini "require a HEX-NET": a HEX-NET without the HN2 hardware has no DoIP either, so
  the reason may not be DoIP.
- Whether a VW gateway asks a third-party tester for anything beyond ISO 13400-2 routing
  activation (an OEM-specific activation type, say).

## 3. CAN FD on the diagnostic CAN

### Cause

**MEB: VW's own self-study programmes say the diagnostic CAN at the OBD socket is CAN FD**
(**sourced**):

- SSP 709 *The ID.3*, technical status 07/2020, "Networking in the ID.3", copy at
  <https://esperformance.net/ssp/vw/SSP_709_EN.pdf>: "The diagnostic CAN with 500/2,000 kbit/sec
  and the Ethernet with 100 Mbit/sec are connected to the diagnostic connector (T16)". Its key
  defines CAN-FD as "CAN with flexible data rate (500 and 2,000 kbit/sec)". The gateway J533 is a
  module inside **ICAS1** (the central computer), still diagnostic address `19`.
- SSP 718 *The ID.4*, technical status 01/2021, "Networking", copy at
  <https://esperformance.net/ssp/vw/SSP_718_EN.pdf>: "Diagnostic CAN/CAN-FD: 500 kbit/s/2 Mbit/s",
  "Diagnostic Ethernet: 100 Mbit/s". The ID.4's network is "largely based on that in the ID.3".

**MQB-evo: the Golf 8's diagnostic CAN is classic, 500 kbit/s** (**sourced**: SSP 706 *The Golf
2020*, technical status 02/2020, "The networking concept",
<https://esperformance.net/ssp/vw/SSP_706_EN.pdf>):

- The new gateway ("Gateway high") has eight CAN connections, four LIN and an internal Ethernet
  module. "Three of the CAN buses have been equipped with a flexible data rate (FD) of up to
  2,000 kBit/sec": powertrain, running gear and driver-assist CAN. The figure gives the
  **Diagnosis CAN bus as 500 kBit/sec**, not among the FD three.
- The Ethernet module has six ports, "one of these leads to the vehicle diagnostic connector",
  100 Mbit/s.
- "A second version of the data bus diagnostic interface (**Gateway low**) **without an Ethernet
  module** will be introduced at a later point in time." Such a Golf 8 has no DoIP at the socket
  at all, so CAN is its only diagnostic path (**inferred** from that sentence).

**MQB-evo at Audi: the A3 8Y's diagnostic CAN is CAN FD** (**sourced**: Audi *Audi A3 (type 8Y)
Self-study programme 680*, pp. 86–88, hosted by NHTSA at
<https://static.nhtsa.gov/odi/tsbs/2021/MC-10194495-0001.pdf>):

- "In the Audi A3 (type 8Y), the CAN FD technology is used for the following CAN bus systems:
  **Diagnostics CAN FD**, Powertrain CAN FD, Running gear CAN FD, Driver assist systems CAN FD."
- Data-phase rate "increased from 500 kbit/s to 2 Mbit/s", payload up to 64 bytes; the data rate
  "is configured in the CAN controller and cannot be modified during operation".
- "CAN FD-compatible controllers can send and receive both standard CAN as well as CAN FD
  transmissions. **Standard CAN controllers can neither send nor receive CAN FD frames.**"
- "Software updates (flashing) are possible both via CAN and Ethernet" — said of the head-up
  display J898 (p. 124), not of the car as a whole. *(Corrected on review: the first draft cited
  "p. 3912 of the text dump, infotainment section" — a line number of a text extract, not a page.)*

This contradicts nothing in SSP 706 outright but disagrees in emphasis: the Golf's figure draws
its diagnosis CAN at 500 kBit/sec without the FD mark. Two MQB-evo cars of the same year, two
readings; whether the Golf 8's diagnostic CAN is FD-capable is **not settled** by these sources.

So the MEB diagnostic bus runs a 500 kbit/s arbitration rate with a 2 Mbit/s data phase, and
DoIP is on the same socket. Both documents label it "CAN/CAN-FD", which reads as a bus that
carries both formats; neither says what frame format the gateway answers a classic request
with, nor whether anything transmits FD frames on it unasked.

**Does a classic request still get a classic answer on those cars? Evidence, all secondary:**

- WiCAN, an **ESP32-C3-based** OBD adapter (its README: "a powerful ESP32-C3-based CAN
  adapter", <https://github.com/meatpiHQ/wican-fw>), ships a profile for VW ID cars
  (`vehicle_profiles/vw/id.json`) that reads battery values (the profile's names: SOC, battery
  temperature) with ELM327 protocol 7 — ISO 15765-4 CAN, 29-bit, 500 kbit/s, classic — from a unit
  at `17FC007B → 17FE007B`, the odometer (`22 295A`) at `17FC0076 → 17FE0076`, the gateway with
  protocol 6 (11-bit, 500 kbit/s) at `710 → 77A` (`22 2AB6` range, `22 2AB2` capacity), and
  `746 → 7B0`. *(Corrected on review: the first draft called `17FC007B` "the battery unit"; the
  profile only names the values, and it left out the `17FC0076` read.)*
- An ESP32 logger for the ID.3, <https://github.com/codingABI/id3esp32obd2>, logs its requests as
  8-byte classic frames: `ID: 17FC007B … Data:03 22 74 48 00 00 00 00`. Its hardware is an
  ESP-WROOM-32 and an **SN65HVD230** — the dash board's transceiver — on pins 6/14, and the sketch
  opens the TWAI in `TWAI_MODE_NORMAL`, 500 kbit/s, accept-all filter (`id3esp32obd2.ino:310-312`).
  Its logged exchanges include a multi-frame answer (`10 13 62 1E 32 …`, flow control `30 00 00`,
  consecutive frames `21 …`, `22 …`) and the VIN from `22 F802` at `17FC0076` (answer starting
  `57 56 57`, "WVW"); answers are padded with `AA`.
- A community table of MEB UDS reads, <https://github.com/spot2000/Volkswagen-MEB-EV-CAN-parameters>,
  lists requests on `17FC007B` padded with `55` and answers on `17FE007B` padded with `AA`
  (also `17FC0076` and `17FC00B9`).

None of these is VW's word, and none says which units failed; but an ESP32's TWAI — the original
ESP32's as much as the C3's (ESP-IDF's TWAI guide for the esp32 has the same "not compatible with
FD format frames" sentence) — cannot receive an FD frame at all, so working MEB profiles are
evidence that at least the MEB gateway and the units at logical `0x76` and `0x7B` answer a classic
500 kbit/s request with classic frames (**inferred**). The id3esp32obd2 sketch adds weak evidence
that nothing sends FD frames on the ID.3's diagnostic CAN while it reads: a classic controller in
**`Normal`** mode with nothing filtered would error-flag each FD frame and climb towards
error-passive; the author reports working reads, but the sketch prints no error counters
(**inferred**, weak).

**Why a classic node matters on an FD bus** (**sourced**): Bosch's CAN FD specification 1.0
(<https://can-newsletter.org/assets/files/ttmedia/raw/e5740b7b5781b8960f55efcc2b93edf8.pdf>,
Recital) says FD and classic controllers work together only "as long as it is not made use of the
CAN FD frame format", and suggests confining FD to modes such as software download while the
controllers without FD "are kept in standby". ESP-IDF: the C3's TWAI "will interpret such frames as
errors"; in listen-only mode it sends no dominant bits, "including ACK and error frames" (TWAI
guide, esp32c3, linked below).

**What our hardware can do** (**sourced** unless marked):

| | CAN FD? | source |
|---|---|---|
| ESP32-C3 TWAI (the board) | **No.** "The TWAI controllers on the ESP32-C3 are not compatible with FD format frames and will interpret such frames as errors." One controller, one mask filter (dual 16-bit mode possible; for 29-bit ids it then filters the upper 16 bits only). | ESP-IDF TWAI guide, esp32c3, <https://docs.espressif.com/projects/esp-idf/en/stable/esp32c3/api-reference/peripherals/twai.html> |
| ESP32 (original), C6, S3, H2, P4 TWAI | No — same sentence on each chip's page (re-checked on review) | ESP-IDF TWAI guide per chip, `https://docs.espressif.com/projects/esp-idf/en/latest/<chip>/api-reference/peripherals/twai.html` with `<chip>` = `esp32`, `esp32c6`, `esp32s3`, `esp32h2`, `esp32p4` |
| **ESP32-C5** (2 controllers), **ESP32-H4** (1) | **Yes**: "also compatible with FD format (a.k.a. CAN FD) frames defined in ISO 11898-1, and can transmit and receive both classic and FD format frames" | <https://docs.espressif.com/projects/esp-idf/en/latest/esp32c5/api-reference/peripherals/twai.html> |
| SN65HVD230 (the board's transceiver) | **No**: "Designed for Data Rates up to 1 Mbps"; TI lists its CAN FD parts separately | <https://www.ti.com/product/SN65HVD230> |
| The owner's CANable: **MKS CANable V2.0 Pro**, STM32G431 + ADM3050E isolated transceiver, firmware `normaldotcom/canable2-fw` (**measured here**, 2026-07-31 bench notes) | **Yes, in hardware and firmware.** canable2-fw "implements non-standard slcan commands to support CANFD messaging": `Y2`/`Y5` data rate 2 or 5 Mbit/s, `d`/`D` FD without bit-rate switch, `b`/`B` FD with it, DLC codes `9`–`F` = 12–64 bytes. It **always** opens the controller in FD+BRS mode (`FrameFormat = FDCAN_FRAME_FD_BRS`) and reports received FD frames as `d…`/`b…` lines for 11-bit ids and `D…`/`B…` for 29-bit ids *(corrected on review: the draft named only `d`/`b`; `src/slcan.c` upper-cases the letter for an extended id)*. Timing is fixed: sample point 88 % in both phases, different prescalers for the two phases, 170 MHz clock, no transmitter-delay compensation set. The transceiver is FD-rated: ADI's data sheet gives the ADM3050E as meeting ISO 11898-2:2016's CAN FD requirements, up to 12 Mbps, maximum loop delay 145 ns (its text as a search index shows it — the PDF timed out here). | <https://github.com/normaldotcom/canable2-fw> README, `src/slcan.c`, `src/can.c`; <https://www.analog.com/media/en/technical-documentation/data-sheets/adm3050e.pdf> |
| candleLight / gs_usb mainline | "STM32G431-based devices (e.g. CANable-MKS 2.0) are not supported by this project yet"; STM32G0B1 (candleLight FD) not in mainline either | <https://github.com/candle-usb/candleLight_fw> README |
| Elmü's CANable 2.5 firmware | slcan **and** candleLight with CAN FD on STM32G431, "tested … on the isolated adapters from MKS Makerbase and Jhoinrch up to 10 Mbaud", settable sample points, acceptance filters, "backward compatible with all legacy software on Windows, Linux, MAC" in legacy mode. Third-party, active (pushed 2026-09-24). | <https://github.com/Elmue/CANable-2.5-firmware-Slcan-and-Candlelight>, its *User & Developer Manual.htm* |

What our code does with an FD frame today (**measured here**): `SlcanBackend`'s reader queues
only lines that start with `t`/`T` (`vag-uds-can/src/slcan.rs:203,229`), so a `d`/`D`/`b`/`B` line
from the CANable is not a frame to it; `CanFrame::new` and `encode_frame` refuse more than 8 bytes;
ISO-TP pads to 8 and has no FD single-frame or first-frame form.

**The reference car's own ODIS project has the CAN FD parameters, unset** (**measured here**,
method in §4): the ISO 15765-3 comparam specification in `SK37X` defines `CP_CANFDBaudrate`
(default 0), `CP_CANFDBitSamplePoint` (80 %), `CP_CANFDSyncJumpWidth` and `CP_CANFDTxMaxDataLength`,
whose default is the enum text "TX_DL = 8, Classic CAN"; neither VW's UDS-on-CAN protocol layer
nor any base variant in the project overrides them. So VW's data model carries FD per logical
link, and this MQB project uses it nowhere.

### Solution

**Reading does not need CAN FD on any car found so far** (**inferred** from the evidence above):
a classic ESP32 TWAI with an SN65HVD230 in `Normal` mode — the dash board's controller class and
transceiver — reads ID.3 units at 500 kbit/s with classic 8-byte frames, 11-bit and 29-bit,
multi-frame answers included; a CAN-only HEX-V2 reads all 35 units of an ID.3 (§2). The MEB
socket is "CAN/CAN-FD": a bus on which FD frames may appear, not one that demands them. What is
needed is to be safe and loud on an FD bus, not to speak FD.

1. **Board: never disturb an FD bus.** Come up `ListenOnly` (ESP-IDF: no dominant bits, no ACK, no
   error frames), watch for bus errors for a moment, go `Normal` only on a bus that reads clean, else
   stay listen-only and put "this car's diagnostic CAN carries CAN FD — the dash cannot read it" on
   the panel. This is `code.md` §8's fix (H1); the reason it is needed is the ESP-IDF sentence
   above — in `Normal`, a classic TWAI answers every FD frame with an error flag until it goes
   error-passive. **S**, firmware only, no RAM worth measuring.
   - **The signal is the bus-error interrupt, not the error counters.** In listen-only mode "the
     error counters will remain frozen" (ESP32-C3 TRM, TWAI chapter, §31.4.1.2 Operation Mode,
     <https://www.espressif.com/sites/default/files/documentation/esp32-c3_technical_reference_manual_en.pdf>),
     and esp-hal 1.0.0-rc.0 — the version the firmware pins — sets the receive error counter to 128
     when it starts a C3 in `ListenOnly`, an errata workaround that keeps the controller error-passive
     so it cannot send a dominant error flag (`esp-hal-1.0.0-rc.0/src/twai/mod.rs:894-917`, local
     cargo registry). A counter check would read the same on a clean bus and an FD bus. The TRM's
     Bus Error Interrupt fires "whenever TWAI controller detects an error on the TWAI bus", with its
     type and bit position in the Error Code Capture register (§31.4.3.7, §31.4.8); that it fires in
     listen-only mode is **inferred** (the controller still receives and checks frames) and must be
     shown on the bench. esp-hal rc.0 enables the bus-error interrupt and reads the capture register
     in its async interrupt handler, but only to cancel a pending transmission; its receive API
     surfaces bus-off and overrun, nothing else (`src/twai/mod.rs:1323-1343,1786-1820`). So the check
     needs that handler to pass the error on — a small patch or wrapper — rather than a second reader
     of a register the handler already consumes. (A reviewer of this note found the counter problem;
     `code.md` §8's "watch the error counters" has it too.)
   - Bench test, no car: the CANable sends FD frames (`b`/`B` lines at `Y2`) as the FD node; the
     check must trip on them and stay quiet on classic traffic.
2. **Laptop: see FD before speaking it.** `SlcanBackend` drops FD lines as non-frames. canable2-fw
   writes them `d`/`b` for 11-bit ids and `D`/`B` for 29-bit ids (`src/slcan.c`: the letter is
   upper-cased for an extended id), so all four count. Count them and report "FD frames on this bus"
   in `dev sniff` and on a timeout, so a car that answers in FD fails with a reason, not as silence.
   **S**, `vag-uds-can/src/slcan.rs`.
3. **Only if a car answers only in FD: FD reads on the laptop.** The hardware is in hand — the
   CANable 2.0 Pro's STM32G431 FDCAN and ADM3050E are FD-capable (table above). The stock
   `canable2-fw` sends and receives FD, but with different nominal and data prescalers, 88 % sample
   points and no transmitter-delay compensation; Elmü's manual says ST recommends one prescaler for
   both phases and reports bus-off "when sending packets with BRS" on a poor choice. Elmü's
   CANable 2.5 firmware (160 MHz clock, same prescaler, settable sample points, silent mode, legacy
   slcan kept) is the better base; it is third-party. At 2 Mbit/s the ADM3050E's ≤ 145 ns loop delay
   is well inside a 500 ns bit, so missing delay compensation is not the problem there
   (**inferred**). Sample points: VW fixes them to SAE J2284 (§2, VW D-PDU notes; J2284-4 is the
   500 k / 2 M part, values not read here); the ODIS comparam defaults are 80 % / 80 % (measured
   here). Code: frames up to 64 bytes through `CanBackend` (a frame-size query), slcan FD lines
   (`d`/`D`/`b`/`B`) encoded and decoded, and ISO 15765-2:2016 framing in `IsoTpCan` — `code.md` §4 (D1). **M**, no
   purchase. Needs a car that answers only in FD — none known.
4. **Board FD, only if the evidence changes.** The C3 cannot, ever (ESP-IDF). Options: an SPI CAN
   FD controller — Microchip MCP2518FD, ISO 11898-1:2015, data up to 8 Mbps, SPI up to 20 MHz
   (<https://ww1.microchip.com/downloads/aemDocuments/documents/OTH/ProductDocuments/DataSheets/External-CAN-FD-Controller-with-SPI-Interface-DS20006027B.pdf>)
   — with a 3.3 V FD transceiver (TI lists TCAN3413/TCAN3414 as the SN65HVD230's FD alternates,
   <https://www.ti.com/product/SN65HVD230>); or an ESP32-C5/H4, whose TWAI handles FD (ESP-IDF;
   esp-hal support for it not checked). Either is a new board revision, a new driver and 64-byte
   frames in RAM (`ram-budget.sh`). **L.** Not recommended now.

### Not known

- **Whether any VAG unit answers a classic request only in FD**, or in FD at all. The MEB evidence
  is classic answers from four addresses — `0x17FE007B`, `0x17FE0076`, `0x77A`, `0x7B0`
  (secondary) — and a HEX-V2 reading a whole ID.3; but Ross-Tech never says whether the HEX-V2
  itself can do FD.
- **Whether any gateway puts FD frames on the diagnostic CAN unprompted** (network management,
  say). On the reference car the one unprompted frame is classic, 29-bit `0x17F00010` at 2 Hz
  (§5); an FD car's equivalent was not observed.
- Whether the A3 8Y's "Diagnostics CAN FD" and the Golf 8's 500 kBit/sec diagnostic CAN differ on
  the wire, and what the PPE and MLB-evo facelift cars use — no source read here covers them.
- The sample points and SJW VW uses at 500 k / 2 M (SAE J2284-4 not read; no newer ODIS project on
  disk), and what `CP_CANFDTxMaxDataLength` newer projects set per logical link.
- Whether the C3's bus-error interrupt fires in listen-only mode; the TRM does not say. The CANable
  bench test in Solution item 1 answers it.

## 4. 29-bit CAN ids

The code side — which layers of `vagcan` hold an 11-bit id — is [`code.md`](code.md) §1 (rows
A1–A6) and is not repeated here. This section answers its question: does a VAG car answer on
29-bit ids at the OBD port, and on which?

### Cause

**VW's diagnostic addressing has a 29-bit half, and the reference car's own platform already
uses it** (**measured here**, 2026-09-28, in the ODIS project `vagcan setup` imported for the
reference car: `SK37X`, project version 2610.2.688, converter 26.1.0.0 —
`~/.vagcan/data/SK37X/sources.json` points at `~/Downloads/SK37X`).

*Method.* An ODIS project gives each base variant its CAN ids as ASAM communication parameters:
`CP_CanPhysReqId` (request), `CP_CanRespUSDTId` (answer), `CP_CanPhysReqFormat` /
`CP_CanRespUSDTFormat` (addressing format) and `CP_CanPhysReqExtAddr` / `CP_CanRespUSDTExtAddr`
(the N_TA byte of extended addressing). In the compiled `0.0.0@BV_*.bv.db` files a parameter
reference is the name's hash twice (the store's DJB2, `vag-data-labels/src/odis/hash.rs`), 17 zero
bytes, `8B`, a type tag and a 4-byte little-endian value — tag `0B` an integer, tag `0E` the hash of
an enum text in `UStringData`. That layout is read off the bytes, not from any document; it is
checked by what it yields: the gateway comes out as `0x710 → 0x77A`, and every pair seen on the
reference car's own bus (`0x70C/776`, `0x70E/778`, `0x714/77E`, `0x715/77F`, `0x74A/7B4`,
`0x74B/7B5`, `0x773/7DD`, `.archive/research/car/other-ecus.md` §1) is in the result. The scripts
live in the session scratchpad and are not committed.

*Result.* All 54 base variants in the project carry a UDS id pair (two of them — brake `0x7E4`,
HVAC `0x7E7` — also carry an ISO 15765-4 pair, **inferred** to be their OBD link). The format column
gives ODIS's enum text for the request side, shortened: the full strings read "normal segmented
11-bit transmit with FC", and so on, with a matching "… receive with FC" for the answer side.

| addressing format (ODIS's enum text, shortened) | pairs | ids | `vagcan` today |
|---|---|---|---|
| "normal segmented 11-bit" | 42 | 39 on `0x70A…0x773 → +0x6A`; `0x7E0`, `0x7E1 → +8`; `0x7F1 → 0x7F9` ("Airba2") | 41 reachable; `0x7F1` fits neither rule |
| "extended segmented 11-bit" | 2 | `0x728 → 0x792`, N_TA `0x0E` (sunroof) or `0x47` (battery monitoring) | no — `IsoTpCan` has no N_TA byte |
| "normal segmented 29-bit" | 8 | `0x17FC0000 + la → 0x17FE0000 + la`, la = `82 83 84 96 97 A0 A9 1602`: light control left/right, sunroof module, light control 2 left/right, sunroof module 2, multifunction module, rain/light sensor | no |
| "extended segmented 29-bit" | 2 | `0x17FC160A/B → 0x17FE160A/B`, N_TA `0x6B` / `0x6C`: LED headlight power modules | no |

So 13 of 54 are out of `vagcan`'s reach on the reference car's own platform. The 29-bit id holds
the logical address in its low 16 bits — `0x17FC0000 | la` to the unit, `0x17FE0000 | la` back — in
ISO 15765-2 **normal** addressing; it is not the `18DA<ta><sa>` "normal fixed" format of legislated
OBD (addressing modes: python-can-isotp's guide,
<https://can-isotp.readthedocs.io/en/latest/isotp/addressing.html>, secondary). Logical addresses
are 16-bit here (`0x1602`, `0x160A`). For OBD, the project's ISO comparam specification keeps the
11-bit defaults (`0x7DF`, `0x7E0 → 0x7E8`) and VW's own OBD protocol layer (`PR_VWOBDOnCAN`) sets
the functional id to `0x700`; no 29-bit OBD id appears.

*Why these units* (**inferred**): under the 11-bit `+0x6A` rule, logical `0x74`/`0x75` would answer
on `0x7DE`/`0x7DF` (the second is the OBD functional id) and `0x76` upward on `0x7E0` upward, the
ISO 15765-4 block. The project's highest 11-bit VW request is `0x773`; units past it, and sub-bus
units with 16-bit addresses, get 29-bit ids.

**MEB puts central units on the same scheme** (**sourced**, secondary — the WiCAN profile and the
id3esp32obd2 logs of §3, spot2000's table): the ID.3 answers at `0x17FC0076 → 0x17FE0076` (the VIN
via `22 F802`, the odometer via `22 295A`) and `0x17FC007B → 0x17FE007B` (battery values, and road
speed `22 F40D` in one byte); spot2000 adds `0x17FC00B9`. The gateway stays 11-bit,
`0x710 → 0x77A`, and so does `0x746 → 0x7B0`. On an ID.3, then, the VIN, the odometer and a road-speed
answer sit behind 29-bit ids (**inferred** from those logs) — which touches `code.md`'s F3 (VIN
from `0x7E0`) and J1 (road speed from `0x7E0`) as well as A1.

**The reference car's bus already carries a 29-bit frame** (**measured here**): `0x17F00010`,
payload `20 10 00 00 00 00 00 80`, at 2 Hz (`other-ecus.md` §1 — "consistent with" a
network-management heartbeat from the gateway, not proven), storming at 3,106 frames/s when
nothing acknowledges it (`research/dash/can-bring-up.md` §5.1). The frame layer handles it;
nothing above is involved.

**The reference car's gateway list, read against the project** (**measured here**, joining
`other-ecus.md` §3's decoded `22 2A26` bitmap with the table): bits
`0A 0C 0E 12 13 14 15 46 4A 4B 67 73` are parking aid, steering column, central electrics, steering assist, brakes, instruments,
airbag, HVAC, driver's door, passenger's door, telematics and information electronics 1 — so the
archive's "unknown" `0x715`, `0x74A`, `0x74B`, `0x773` are the airbag, the two front doors and the
infotainment unit. Bits `76` and `77` match no base variant in any format: the project gives a
tester no address for them. Bit `00` matches the functional request id VW's protocol layer sets,
`0x700` (`CP_CanFuncReqId`; the ISO default is `0x7DF`). The car lists no unit that the project
puts on 29-bit.

### Solution

1. **Take each unit's ids from the car's own project, not from rules.** At `vagcan setup`, read
   the five parameters above per base variant and cache them in `cache.sqlite` beside the labels.
   That is per-car data, as `CLAUDE.md` asks — on this path no id and no id rule is a constant in
   the code. A unit found in the gateway's list is matched to base variants by logical address (the
   low bits of the id); its `F19E`, read once it is addressed, confirms the variant. In
   `vag-data-labels`' ODIS reader this is one more loader over members it already inflates. **S–M**,
   no car needed to build it.
   - It settles `code.md` §1's step 3 ("which scheme a car uses is read"): the scheme is in the
     project. A probe of the gateway under each known scheme stays as the fallback for a car with no
     project.
   - A VCDS-only owner has no such parameters. For them the fallback is a platform rule with its
     evidence beside it, as `+0x6A` is today in `address.rs`: `0x17FC0000 | la → 0x17FE0000 | la`
     for every unit the 11-bit rule cannot hold — a property of VW's addressing (measured on the MQB
     project, seen on MEB), not of one car; still **inferred** as a rule, and one read per car
     confirms it.
2. **Carry 29-bit ids above the frame** — `code.md` §1 steps 2, 4, 5 (A1, A4–A6): a `CanId` in the
   scheduler's unit, the dash plan and the guard; link v2 to the board; the board's filter switched
   to an extended filter for an exchange whose answer id is 29-bit (ESP-IDF: dual-filter mode sees
   only the upper 16 bits of a 29-bit id, so single-filter mode). **M.**
3. **ISO-TP extended addressing** for the four units that use it on the reference platform: an
   addressing-format field in `IsoTpCan` — the N_TA byte first in every frame, single frames up to 6
   bytes, consecutive frames 6 bytes, the answer's first byte checked against the unit's N_TA — set
   from the project's format and ext-addr parameters. Two units share `0x728/0x792` and differ only
   in that byte. **S**, `vag-uds-can/src/isotp.rs`.
4. **Airbag 2** (`0x7F1 → 0x7F9`) comes with item 1; no new rule.

Guards: the same allowlisted services go to more ids. A unit the gateway lists is asked as listed
units are today. A unit known only from the project — the reference car's list names none of its
29-bit units — is a candidate asked blind, which is a sweep of addresses and goes through the guard
`survey` has, as `code.md` §2 already says for candidates from the label data.

Cost: S–M (items 1, 3) plus M (item 2). Needs a car: yes — one read at a 29-bit unit. The reference
car lists none, apart from the unexplained bits `76`/`77`; an MEB car's `0x17FC0076` is the cheapest
first read.

### Not known

- How VCDS's four-digit addresses (`8105`, `8107`, `81xx`, `C002`) map to logical addresses and
  ids. The project's 16-bit logical addresses (`0x1602`, `0x160A`) on 29-bit ids fit; the mapping is
  not established. VCDS 18.9 (Sep 2018) added "Support for new 16-bit '5-baud' control module
  addresses".
- Whether a gateway's `2A26` bitmap lists 29-bit units at all — the reference car lists none that
  the project puts on 29-bit, and may simply have none.
- What the reference car's bits `76`/`77` are. One `22 F187` at `0x17FC0076` and `0x17FC0077`
  settles it (`code.md` §2, B4).
- Which units MEB's `0x76` and `0x7B` are; the logs read the VIN and odometer from one and battery
  values and road speed from the other, and neither names the unit.
- How many more units newer projects (MQB-evo, MEB, PPE) put on 29-bit — likely more (**inferred**),
  not measured: no newer project is on disk.

## 5. Other at this layer

The numbers below marked **measured here** come from the reference car's ODIS project, read as in
§4: three layers of communication parameters — the ASAM comparam specification
(`ISO_15765_3_on_ISO_15765_2.cp.db`, defaults), VW's UDS-on-CAN protocol layer
(`PR_UDSOnCAN.pr.db`, VW's choices for every unit) and each base variant (`BV_*.bv.db`, per unit).
ASAM parameters count time in microseconds; they are given here in ms or s.

### 5.1 Gateway behaviour

**What is known.**

- **Every request goes through the gateway, and it routes to the car's buses** (**sourced**; the
  SSP URLs are in §1–§3 and in Sources):
  Audi's firewall schematic has the diagnostic connector on J533 and the units behind it, and "the
  speed at which the data could be transmitted across the gateway" is what DoIP was added for
  (SSP 970293, pp. 40–41); the e-tron GT's J533 is connected to every bus but the
  information-electronics CAN and MOST, and lists "Network system gateway", "Diagnostic master",
  "Manages diagnostics firewall" among its tasks (SSP 684, p. 140); the ID.3's J533 lives in ICAS1,
  which "also contains further addressable modules" (SSP 709). Ethernet-hosted units answer a CAN
  tester through it (§2).
- **The gateway keeps its 11-bit address on MEB** (**sourced**, secondary): the ID.3's gateway
  answers `22 2AB2` at `0x710 → 0x77A` (id3esp32obd2 log; WiCAN profile). Whether an `ICAS1` or
  `GW2020` gateway answers `22 2A26`, the list `vagcan` walks, no source says — `code.md` §2's open
  question, one read. VCDS 21.9's "Coding for Gateway Installation List enhanced" shows the
  installation list still exists on gateways of that time (**inferred**).
- **VW budgets time for the gateway hop** (**measured here**): `CP_CanTransmissionTime` is 150 ms
  in VW's protocol layer (ASAM default 100 ms), and VW's tester waits P2 = 50 ms + 150 ms = 200 ms
  (§5.3). The headlight range control unit (`0x754`) gets 450 ms — **inferred** to mean it sits
  behind a further hop.
- **VW's functional request id is `0x700`** (**measured here**: `CP_CanFuncReqId` in the protocol
  layer and in one vehicle-info object; two others set `0x703` and the OBD `0x7DF`; the ASAM default
  is `0x7DF`). That is the id VCDS was seen sending TesterPresent to (`other-ecus.md` §1), and the
  reference car's `2A26` bit `00`.
- **Slow starters behind the gateway** (**sourced**: SSP 718,
  <https://esperformance.net/ssp/vw/SSP_718_EN.pdf>): the ID.4's ICAS3 hosts two virtual units, and
  the functions with "a longer start-up time" are grouped into the second — a unit that may miss a
  first request after wake-up (**inferred**).
- **The OBD-port bus is quiet** on the reference car (**measured here**): diagnostic traffic and
  `0x17F00010` at 2 Hz, nothing else (`other-ecus.md` §1). Not measured on a newer car.
- **Ross-Tech's legacy interfaces stop at MY 2019** (**sourced**, current.php: none of them "will
  work properly with 2019 or newer model cars"; reason not given). The same months brought VCDS 18.9
  (Sept 2018: "Support for model year 2019 cars", "new 16-bit '5-baud' control module addresses")
  and later "New Codeblock for HEX-NET/HEX-V2 … with new addresses" (21.3). An address table in the
  current interfaces' firmware that the legacy ones cannot take is the reading that fits
  (**inferred**); nothing in the sources points at a change of bit rate or frame format.
- The diagnostic firewall and filter: §1.

**Solution.** Nothing new at the gateway beyond §1 (report the filter) and `code.md` §2 (walk
logical addresses, try `2A26` then `04A3`, degrade instead of failing). Take each unit's first-answer
wait from the project (§5.3) rather than a constant, so a unit behind a slow hop is not dropped.

**Not known.** Whether a newer gateway rate-limits diagnostic requests (`code.md` §9 found nothing
either); whether it forwards anything unprompted onto the diagnostic CAN.

### 5.2 Bus speed

**What is known.** Every diagnostic CAN in the sources runs its arbitration phase at 500 kbit/s:
A8 4N and e-tron GT (bus tables, "Diagnostics CAN … 500 kbit/s"), Golf 8 ("Diagnosis CAN bus 500
kBit/sec"), ID.3 and ID.4 (500 / 2,000 kbit/s, the second the FD data phase), A3 8Y (data phase
"increased from 500 kbit/s to 2 Mbit/s"). The A8's figure drawing "Diagnostics CAN 1 MBit/s" is the
ceiling that motivates DoIP, not the rate (§2). **Measured here**: the project's `CP_Baudrate` is
500,000 with an 80 % sample point, and nothing overrides it. **Measured here** (bench notes): the
CANable runs 500 kbit/s at an 88 % sample point and reads the reference car; the board runs esp-hal's
`B500K`.

**Solution.** None needed: keep 500 kbit/s on both transports, no auto-baud. Open listen-only first
and look for bus errors before going `Normal` (§3, with the bus-error interrupt as the signal) — the
same check that protects an FD bus protects a bus at a rate the car does not use.

**Not known.** The data-phase sample point and SJW VW uses at 2 Mbit/s (§3).

### 5.3 P2 / P2\*

**What is known** (**measured here** unless marked):

| wait | ASAM default | VW protocol layer | per unit (SK37X) | `vagcan` |
|---|---|---|---|---|
| unit's own P2 (`CP_P2Max_Ecu`) | 50 ms | — | — | — |
| tester's first-answer wait (`CP_P2Max`) | 150 ms | **200 ms** | rear door units (`0x73E`, `0x73F`): **1.9 s** | 500 ms per scheduled read; **300 ms** identification probe; 2 s raw exchange; board 500 ms |
| unit's own P2\* (`CP_P2Star_Ecu`) | 5 s | — | — | — |
| tester's wait after `7F xx 78` (`CP_P2Star`) | 5.05 s | **5.15 s** | telematics: 400–600 s | **5 s** each, up to 30 (laptop); 5 s each, 10 s in all (board) |
| total time a unit may keep answering `78` (`CP_RC78CompletionTimeout`) | 25 s | 100 s | instruments 900 s; front camera, image processing 480 s | 150 s (laptop, 30 × 5 s); 10 s (board) |
| hop allowance (`CP_CanTransmissionTime`) | 100 ms | 150 ms | headlight range control 450 ms | — |
| repeats of a failed request (`CP_RepeatReqCountApp`) | 0 | 2 | — | no immediate repeat; the scheduler asks again on its next period, with backoff |
| what the tester does on `78` (`CP_RC78Handling`) | "Continue unlimited" | "Continue until RC78 timeout" | — | up to 30 pendings (laptop), 10 s (board) |

`vagcan`'s values are from `code.md` §5. The ISO 14229-2 figures `vagcan` cites (P2 50 ms, P2\* 5 s)
are the units' own limits. VW's tester waits are those plus its 150 ms hop allowance (200 ms,
5.15 s); ASAM's defaults add 100 ms to P2 and 50 ms to P2\*.

**What it means** (**inferred**):

- **The 300 ms probe drops slow units.** A rear door unit may take up to 1.9 s by its own data;
  the identification probe (`vag-cli-core/src/units.rs`) gives it 300 ms and then drops it from
  `watch`/`measure` without a word (`code.md` §5, E1). The ODIS project **does** carry per-unit P2/P2\* — the question `code.md` §5
  left to `labels.md` is answered here.
- **`vagcan`'s wait after `78` (P2\*) is 150 ms short of VW's.** An answer 5.0–5.15 s after a
  `78` is lost. Rare for reads; cheap to fix.
- The board's 10 s cap after `78` is shorter than any VW figure; for `22`/`19` reads that is a
  choice, not a bug (a read that pends ten seconds is not a dash value).

**Solution.** Per-unit P2/P2\* from the project, cached at `setup` like the CAN ids (§4); VW's
protocol values (200 ms / 5.15 s) as the default for a unit the project does not override; retry the
identification probe once with the unit's own P2 before dropping a listed unit (`code.md` §5
suggests P2\*; P2 is the wait that bounds a first answer, and the project now gives it per unit).
**S.** Needs a car only to observe a slow unit.

**Not known.** Whether the rear-door 1.9 s applies to a plain `22` read or exists for some routine;
how newer units behave.

### 5.4 ISO-TP payload length and flow control

**What is known.**

- Classic ISO-TP carries at most 4,095 bytes (12-bit first-frame length); ISO 15765-2:2016 adds an
  escape to a 32-bit length and CAN FD framing for single and first frames (**sourced**, secondary:
  <https://en.wikipedia.org/wiki/ISO_15765-2>). `vagcan` stops at 4,095 and has no FD framing
  (`code.md` §4, D1/D2).
- The largest answer on record for the reference car is its body control module's reply to
  `19 02` with status mask `0xFF`: 508 codes, 3 + 4 × 508 = 2,035 bytes
  (`vag-uds-client/src/faultcount.rs:18-23`, `code.md` §4) — inside the classic limit. Newer cars:
  not measured.
- **Measured here**, VW's flow-control settings against `vagcan`'s:

  | parameter | ASAM default | VW protocol layer / unit | `vagcan` |
  |---|---|---|---|
  | FC.WAIT frames tolerated (`CP_CanMaxNumWaitFrames`) | 255 | **15** | **8** (`isotp.rs:15`) |
  | block size / STmin the tester asks (`CP_BlockSize`, `CP_StMin`) | 0 / 0 | not overridden | 0 / 0 |
  | N_Bs, N_Cr (`CP_Bs`, `CP_Cr`) | 1 s / 1 s | not overridden | N_Bs 1 s |
  | padding byte (`CP_CanFillerByte`) | `0x55` | not overridden | `0x00` |
  | STmin forced on the tester (`CP_StMinOverride`) | none | sunroof: 1 ms | the unit's own STmin |
  | FD frame size (`CP_CANFDTxMaxDataLength`) | "TX_DL = 8, Classic CAN" | not overridden | 8 |

**What it means** (**inferred**): no blocker on classic CAN. The FC.WAIT limit matters only when
`vagcan` sends a multi-frame request — a `22` with four or more identifiers, which the scheduler
builds (up to eight) — and a unit then asks for more than 8 waits; VW's tester would accept up to 15.
Padding: MEB units answered requests padded with `0x00` (id3esp32obd2), so `0x55` is VW's habit, not
a requirement.

**Solution.** The first-frame escape (~30 lines, `code.md` §4); N_WFTmax 15, or the project's value;
the padding byte from the project if a unit is ever seen to care. FD framing only with FD (§3).
**S.** No car needed.

**Not known.** The largest answer on a newer car; whether any newer project sets
`CP_CANFDTxMaxDataLength` above 8 for a link a classic tester uses.

## Corrections made on review (2026-09-28)

The first draft (`ed680b1`) was kept and its sources re-read; seven statements in it were wrong,
incomplete or ambiguous and are corrected in place, each marked *(corrected on review …)*:

1. §1 — the diagnostic filter: VCDS "may be" in restricted mode and "uses a workaround"; the draft
   said "only" in restricted mode and that Ross-Tech does not say how.
2. §2 — HEX-V2's exception list also names Lamborghini ("these require a HEX-NET").
3. §3 — SSP 680's "via CAN and Ethernet" is p. 124 and about the head-up display J898; the draft
   cited a text-dump line number as a page, for the whole car.
4. §3 — WiCAN's `17FC007B` read: the profile names battery values, not "the battery unit"; the
   `17FC0076` odometer read was left out.
5. Baseline table, whole-car walk: the laptop decodes every bit of the list, only the board's fault
   count stops at the VW block; the draft said "VW block bits only" for both.
6. Baseline table, UDS timing: "10 × P2 = 50 ms" read as P2 = 5 ms; reworded to "10 × a P2 of
   50 ms".
7. §3, hardware table: canable2-fw reports FD frames as `d`/`b` for 11-bit ids and `D`/`B` for
   29-bit ids; the draft named only `d`/`b`.

Everything else in the draft checked out against its sources — SSP 664 through its English
edition (970293), which says the same. The summary table and every `_todo_` are now filled.

## Sources

All read 2026-09-28. ISO 14229-1/-2, ISO 15765-2/-4, ISO 13400 and ISO 11898-1 are cited by number
only: iso.org answered with a bot check, so their content here comes from the secondary sources
marked as such, or from ODIS's comparam specification (measured here).

**VW and Audi**

- SSP 706 *The Golf 2020* (02/2020) — networking, SFD — <https://esperformance.net/ssp/vw/SSP_706_EN.pdf>
- SSP 709 *The ID.3* (07/2020) — <https://esperformance.net/ssp/vw/SSP_709_EN.pdf>
- SSP 718 *The ID.4* (01/2021) — <https://esperformance.net/ssp/vw/SSP_718_EN.pdf>
- Audi SSP 680 *Audi A3 (type 8Y)*, pp. 86–88, 124 — <https://static.nhtsa.gov/odi/tsbs/2021/MC-10194495-0001.pdf>
- Audi eSelf-Study 970293 *The 2019 Audi A8 Electrics and Electronics*, pp. 33, 40–41 — <https://static.nhtsa.gov/odi/tsbs/2018/MC-10144683-9999.pdf>
- Audi SSP 664 *Audi A8 (Typ 4N) Elektrik und Elektronik* (German original of the above; not re-read on review, its content checked against the English edition) — <https://pdfcoffee.com/ssp664-wg-de-pdf-free.html>
- Audi SSP 684 *Audi e-tron GT (type F8)*, pp. 130, 140 — <https://static.nhtsa.gov/odi/tsbs/2021/MC-10190946-0001.pdf>
- Volkswagen AG, *Schutz der Fahrzeugdiagnose (SFD) — Nutzungsbedingungen*, Stand Juli 2021 — <https://privacy.volkswagen.com/download/get-document-content/57b3edcf-a670-4cfc-87b5-f6afd70f3bb2>
- **Local, measured here:** the reference car's ODIS project `SK37X` 2610.2.688 (`~/Downloads/SK37X`, imported per `~/.vagcan/data/SK37X/sources.json`): `ISO_15765_3_on_ISO_15765_2.cp.db`, `ISO_OBD_on_ISO_15765_4.cp.db`, `PR_UDSOnCAN.pr.db`, `PR_VWOBDOnCAN.pr.db`, `VI_SK37X.vi.db`, `FG_*.fg.db`, `BV_*.bv.db`, string pools `AStringData`/`UStringData`.
- **Local, measured here:** VW D-PDU API 31.0.0 release notes (17.12.2024), `~/Downloads/D-PDU_API_31.0.0/D-PDU_API_31.0.0/VW_D-PDU_API_31.0.0_Releasenotes.pdf` — marked internal and confidential; summarised, not copied.

**Ross-Tech**

- Wiki, "SFD", revision 9826 — <https://wiki.ross-tech.com/wiki/index.php?title=SFD&oldid=9826>
- VCDS current release (26.9) — <https://www.ross-tech.com/vcds/download/current.php>
- VCDS revision history — <https://www.ross-tech.com/vcds/revisions.php>
- HEX-V2 — <https://www.ross-tech.com/vcds/hex-v2.php>; store page — <https://store.ross-tech.com/shop/vchv2_ent/>
- HEX-NET — <https://www.ross-tech.com/vcds/hex-net.php>
- Forum Auto-Scans: ID.3 over HEX-V2 — <https://forums.ross-tech.com/index.php?threads/25398/>; ID.3 over HEX-NET2 — <https://forums.ross-tech.com/index.php?threads/24641/>; Golf 8 over HEX-NET2 — <https://forums.ross-tech.com/index.php?threads/40985/>

**Chips, controllers, CAN**

- Espressif, ESP-IDF TWAI guide: ESP32-C3 — <https://docs.espressif.com/projects/esp-idf/en/stable/esp32c3/api-reference/peripherals/twai.html>; ESP32-C5 — <https://docs.espressif.com/projects/esp-idf/en/latest/esp32c5/api-reference/peripherals/twai.html>; ESP32-H4 — <https://docs.espressif.com/projects/esp-idf/en/latest/esp32h4/api-reference/peripherals/twai.html>; ESP32 — <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/peripherals/twai.html>; C6, S3, H2, P4 — the same path under `latest/esp32c6`, `esp32s3`, `esp32h2`, `esp32p4`
- Espressif, *ESP32-C3 Technical Reference Manual*, TWAI chapter (§31.4.1.2 modes, §31.4.3.7 Bus Error Interrupt, §31.4.8 Error Code Capture) — <https://www.espressif.com/sites/default/files/documentation/esp32-c3_technical_reference_manual_en.pdf>
- esp-hal `1.0.0-rc.0` (the version `vag-dash-fw` pins), `src/twai/mod.rs:894-917,1323-1343,1786-1820` — local cargo registry
- Espressif, ESP-IDF Ethernet guide for ESP32-C3 — <https://docs.espressif.com/projects/esp-idf/en/stable/esp32c3/api-reference/network/esp_eth.html>
- TI SN65HVD230 — <https://www.ti.com/product/SN65HVD230>
- Analog Devices ADM3050E data sheet — <https://www.analog.com/media/en/technical-documentation/data-sheets/adm3050e.pdf> (its figures as a search index shows them; the PDF timed out here, so they are not re-read)
- Microchip MCP2518FD data sheet DS20006027B — <https://ww1.microchip.com/downloads/aemDocuments/documents/OTH/ProductDocuments/DataSheets/External-CAN-FD-Controller-with-SPI-Interface-DS20006027B.pdf>
- Bosch, *CAN with Flexible Data-Rate, Specification Version 1.0* (2012) — <https://can-newsletter.org/assets/files/ttmedia/raw/e5740b7b5781b8960f55efcc2b93edf8.pdf>
- SAE J2284-4, title and scope only — <https://saemobilus.sae.org/standards/j22844_202211-high-speed-hsc-vehicle-applications-500-kbps-fd-data-2-mbps>

**CANable and candleLight**

- `normaldotcom/canable2-fw`: README, `src/can.c`, `src/slcan.c` — <https://github.com/normaldotcom/canable2-fw>
- `candle-usb/candleLight_fw` README — <https://github.com/candle-usb/candleLight_fw>
- Elmü, CANable 2.5 firmware, README and *User & Developer Manual.htm* — <https://github.com/Elmue/CANable-2.5-firmware-Slcan-and-Candlelight>

**Secondary (community and reference)**

- WiCAN firmware, README and `vehicle_profiles/vw/id.json`, `ev_meb.json` — <https://github.com/meatpiHQ/wican-fw>
- id3esp32obd2, README and sketch (`id3esp32obd2.ino`, `requests.ino`) — <https://github.com/codingABI/id3esp32obd2>
- spot2000, VW MEB UDS PID list — <https://github.com/spot2000/Volkswagen-MEB-EV-CAN-parameters>
- python-can-isotp, addressing modes — <https://can-isotp.readthedocs.io/en/latest/isotp/addressing.html>
- python-doipclient, automotive Ethernet primer (ISO 13400-4 pin options) — <https://python-doipclient.readthedocs.io/en/stable/automotive_ethernet.html>
- Wikipedia, ISO 15765-2 — <https://en.wikipedia.org/wiki/ISO_15765-2>

**This repository**

- [`code.md`](code.md) §1, §2, §4, §5, §8, §9 (and its summary rows A1–A6, B4, D1, D2, E1, F3, H1, J1) — the code side
- [`labels.md`](labels.md) — the label data; `CLAUDE.md` (safety, data rules, tech stack); `ARCHITECTURE.md` ("The USB cable")
- `.archive/research/car/other-ecus.md` §1, §3 — the reference car's bus and gateway list
- `research/dash/can-bring-up.md` §4.3, §5, §5.1 — the board's hardware and the `0x17F00010` storm
- Code, in `crates/`: `uds/vag-uds-transport/src/frame.rs`; `uds/vag-uds-can/src/{backend,slcan,isotp,filter}.rs`; `uds/vag-uds-client/src/{address,gateway,faultcount,uds_async}.rs`; `cli/vag-cli-core/src/{device,units}.rs`, `cli/vag-cli-core/src/bus/mod.rs`; `cli/vag-cli-diag/src/{faults,safety}.rs`; `data/vag-data-labels/src/odis/hash.rs`; `dash/vag-dash-fw/src/bin/dash.rs`, `dash/vag-dash-fw/ram-budget.sh`
