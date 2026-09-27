# Newer VAG cars — protocol, transport and hardware

Status: **in progress** (2026-09-28). Research only — no code, no car, no adapter, no BLE.

Question (owner, 2026-09-28): what stops `vagcan` reading cars from about 2020 on
(MQB-evo, MEB, MLB-evo/PPE), and what the fix is. This file is the protocol / transport /
hardware part. Label data and hard-coded assumptions in the code are covered by sibling
files in this directory.

Every claim is marked **sourced** (with URL), **measured here**, or **inferred**.

## Summary

| blocker | which cars | blocks reading? | confidence | solution | cost | needs a car? |
|---|---|---|---|---|---|---|
| SFD | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ |
| DoIP on the OBD port | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ |
| CAN FD on the diagnostic CAN | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ |
| 29-bit CAN ids | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ |
| Other (gateway, speed, timing, ISO-TP length) | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ |

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
| UDS timing | a scheduled read waits 500 ms (10 × P2 = 50 ms), `7F xx 78` extends by 5 s (ISO 14229-2 default P2*), at most 30 pendings; a raw exchange waits 2 s | `vag-cli-core/src/bus/mod.rs:60-72,396`, `vag-uds-client/src/uds_async.rs` |
| sessions | reads run in the **default** session; `10 03` (extended) is opt-in and refused on a moving car; the board refuses `10 02` | `vag-cli-diag/src/safety.rs`, `ARCHITECTURE.md` "The USB cable" |
| whole-car walk | gateway `0x710 → 0x77A`, installation list `22 2A26`, VW block bits only | `vag-uds-client/src/gateway.rs` |
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
- **Newer models add a diagnostic filter** that, in Ross-Tech's words, can block diagnostic
  access to some control modules entirely. VCDS then reaches those modules only in a
  **"Restricted (read-only) mode"**, marked `-R` in the Auto-Scan (`VCID: …-R`, and
  `-R SFD+SFD2` combined). Ross-Tech does not say how VCDS reaches them.
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
(see §5). Its store page adds that HEX-V2 is "Not compatible with Lamborghini, Routan, Transporter
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

_todo_

### Not known

_todo_

## 2. DoIP on the OBD port

### Cause

**Ross-Tech's classic-CAN interface still claims every current VAG car** (**sourced**):

- HEX-V2 product page, <https://www.ross-tech.com/vcds/hex-v2.php> (read 2026-09-28): compatible
  with "all diagnostic-capable VW/Audi passenger cars from 1996 to current; K, K+L, dual-K, or
  CAN". Its exception list names only Routan vans and some 1991–94 Audi TDIs. HEX-V2 has no
  DoIP.
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
- ID.3 (SSP 709, 07/2020) and ID.4 (SSP 718, 01/2021): "The diagnostic CAN … and the Ethernet
  with 100 Mbit/sec are connected to the diagnostic connector (T16)"; "Diagnostic Ethernet:
  100 Mbit/s".

So DoIP exists car-side on VAG cars since MLBevo (A8 4N, 2017; the wires since the Q7 4M,
2015), it was added beside the diagnostic CAN rather than in place of it, and Ross-Tech sells a
CAN-only interface as covering every current car, Golf 8 and ID.x included (**inferred** from
the pages together; neither source says in one sentence "CAN on pins 6/14 answers on every DoIP
car").

### Solution

_todo_

### Not known

_todo_

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
- Software updates "are possible both via CAN and Ethernet" (p. 3912 of the text dump,
  infotainment section).

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
  (`vehicle_profiles/vw/id.json`) that reads the battery unit with ELM327 protocol 7 — ISO 15765-4
  CAN, 29-bit, 500 kbit/s, classic — at `17FC007B → 17FE007B`, and the gateway with protocol 6
  (11-bit, 500 kbit/s) at `710 → 77A` (`22 2AB6` range, `22 2AB2` capacity), plus `746 → 7B0`.
- An ESP32 logger for the ID.3, <https://github.com/codingABI/id3esp32obd2>, logs its requests as
  8-byte classic frames: `ID: 17FC007B … Data:03 22 74 48 00 00 00 00`.
- A community table of MEB UDS reads, <https://github.com/spot2000/Volkswagen-MEB-EV-CAN-parameters>,
  lists requests on `17FC007B` padded with `55` and answers on `17FE007B` padded with `AA`.

None of these is VW's word, and none says which units answered or failed; but an ESP32-C3's TWAI
cannot receive an FD frame at all (below), so a working WiCAN profile is evidence that at least
the MEB gateway and battery unit answer a classic 500 kbit/s request with classic frames
(**inferred**).

**What our hardware can do** (**sourced** unless marked):

| | CAN FD? | source |
|---|---|---|
| ESP32-C3 TWAI (the board) | **No.** "The TWAI controllers on the ESP32-C3 are not compatible with FD format frames and will interpret such frames as errors." One controller, one mask filter (dual 16-bit mode possible; for 29-bit ids it then filters the upper 16 bits only). | ESP-IDF TWAI guide, esp32c3, <https://docs.espressif.com/projects/esp-idf/en/stable/esp32c3/api-reference/peripherals/twai.html> |
| ESP32-C6, S3, H2, P4 TWAI | No — same sentence on each chip's page | ESP-IDF TWAI guide, `latest`, per chip |
| **ESP32-C5** (2 controllers), **ESP32-H4** (1) | **Yes**: "also compatible with FD format (a.k.a. CAN FD) frames defined in ISO 11898-1, and can transmit and receive both classic and FD format frames" | <https://docs.espressif.com/projects/esp-idf/en/latest/esp32c5/api-reference/peripherals/twai.html> |
| SN65HVD230 (the board's transceiver) | **No**: "Designed for Data Rates up to 1 Mbps"; TI lists its CAN FD parts separately | <https://www.ti.com/product/SN65HVD230> |
| The owner's CANable: **MKS CANable V2.0 Pro**, STM32G431 + ADM3050E isolated transceiver, firmware `normaldotcom/canable2-fw` (**measured here**, 2026-07-31 bench notes) | **Yes, in hardware and firmware.** canable2-fw "implements non-standard slcan commands to support CANFD messaging": `Y2`/`Y5` data rate 2 or 5 Mbit/s, `d`/`D` FD without bit-rate switch, `b`/`B` FD with it, DLC codes `9`–`F` = 12–64 bytes. It **always** opens the controller in FD+BRS mode (`FrameFormat = FDCAN_FRAME_FD_BRS`) and reports received FD frames as `d…`/`b…` lines. Timing is fixed: sample point 88 % in both phases, different prescalers for the two phases, 170 MHz clock, no transmitter-delay compensation set. | <https://github.com/normaldotcom/canable2-fw> README, `src/slcan.c`, `src/can.c` |
| candleLight / gs_usb mainline | "STM32G431-based devices (e.g. CANable-MKS 2.0) are not supported by this project yet"; STM32G0B1 (candleLight FD) not in mainline either | <https://github.com/candle-usb/candleLight_fw> README |
| Elmü's CANable 2.5 firmware | slcan **and** candleLight with CAN FD on STM32G431, "tested … on the isolated adapters from MKS Makerbase and Jhoinrch up to 10 Mbaud", settable sample points, acceptance filters, "backward compatible with all legacy software on Windows, Linux, MAC" in legacy mode. Third-party, active (pushed 2026-09-24). | <https://github.com/Elmue/CANable-2.5-firmware-Slcan-and-Candlelight>, its *User & Developer Manual.htm* |

What our code does with an FD frame today (**measured here**): `SlcanBackend`'s reader queues
only lines that start with `t`/`T` (`vag-uds-can/src/slcan.rs:203,229`), so a `d`/`b` line from
the CANable is not a frame to it; `CanFrame::new` and `encode_frame` refuse more than 8 bytes;
ISO-TP pads to 8 and has no FD single-frame or first-frame form.

### Solution

_todo_

### Not known

_todo_

## 4. 29-bit CAN ids

### Cause

_todo_

### Solution

_todo_

### Not known

_todo_

## 5. Other at this layer

_todo — gateway behaviour, bus speed, P2/P2*, ISO-TP payload length._

## Sources

_todo_
