# dash / 11 — BLE on the C3: what it carries, and what it is for

**Subsystem:** dash · **Needs the car:** no · **Proven on hardware 2026-08-25**

`09` was built on Bluetooth Classic SPP, which the C3 does not have ([`10-c3-recon.md`](10-c3-recon.md)).
This is what BLE does instead — measured on the board, not read off a datasheet — and
what its job is now that the wireless-adapter path has moved to Wi-Fi TCP.

## The decision: BLE is the configuration link, not the data link

Settled with the owner 2026-08-25. The laptop connects to the device to **configure the
panel** — what to show and how. Small, structured, rare. Not a stream.

That is a good fit, and the same reasoning disqualifies BLE from `09`'s job. A BLE
connection interval is 7.5–30 ms and one ATT exchange carries an MTU's worth, so the
useful rate is single-digit to low tens of KB/s. CAN at 500 kbit/s under load is on the
order of 60 KB/s of raw frames, and slcan is ASCII, so two to three times that again.
**BLE cannot carry a busy bus; Wi-Fi TCP carries it with room to spare.** Configuration
is a few kilobytes, once — the same limit that disqualifies it there is irrelevant here.

## What is on the board

`~/esp/c3-recon/ble` (outside the repo; the checkout keeps no firmware), two binaries:

- **`peri`** — advertises as `vagcan-dash`, serves three services, echoes what it is
  sent. This is what was verified.
- **`scan`** — the central role: scan the air and print addresses with RSSI. Built,
  **not yet flashed** — flashing it replaces `peri` on the single board.

Versions that work together, which is not the set `esp-generate` produces (see below):
`esp-hal 1.0.0-rc.0`, `esp-wifi 0.15.1`, **`trouble-host 0.2.4`**, **`bt-hci 0.3.2`**,
`esp-hal-embassy 0.9.1`, `embassy-executor 0.7`, `embassy-sync 0.7.2`, `esp-alloc 0.8`.

## BLE has no SPP, and no profile puts a device in the phone's settings

This is the expectation that has to be corrected before any design rests on it.
Classic Bluetooth had profiles the OS itself knows how to use — SPP, A2DP, HFP. BLE
replaced all of that with GATT, and the system Bluetooth screen serves exactly one class
of BLE device:

| | generic BLE peripheral | HID over GATT |
|---|---|---|
| **iOS** Settings → Bluetooth | not listed at all | listed, pairs, works |
| **macOS** System Settings | not listed at all | listed, pairs, works |
| **Android** "Pair new device" | listed; tapping starts bonding, which needs SMP and then does nothing useful | listed, pairs, works |

So "give it a profile so it connects from the phone" means **HOGP or nothing**; the
specification provides no middle option. HOGP costs service `0x1812` with a HID report
map, protocol mode and control point, plus mandatory encryption and bonding — and buys
a device that pretends to be a keyboard, which a dash is not. Not done, and not needed:
the laptop talks to it with code, which is the case that matters.

## The three services on the device

| service | UUID | why |
|---|---|---|
| Device Information | `0x180A` | manufacturer / model / firmware revision. Every scanner and OS reads it; it costs three constant strings and the device stops being anonymous |
| Battery | `0x180F` | notifies a level. A standard profile with a visible effect — and where `08`'s 12 V rail measurement will eventually surface |
| Nordic UART | `6e400001-b5a3-f393-e0a9-e50e24dcca9e` | the pipe. `…0002` is written by the central, `…0003` notifies back |

**NUS is not a SIG profile** — it is Nordic's convention, and it won by being what every
BLE terminal application implements. It is the honest replacement for SPP: two
characteristics standing in for two directions of a serial port.

**L2CAP connection-oriented channels were considered and rejected.** They would give a
real credit-managed stream instead of hand-rolled chunking, `trouble-host` supports them,
and CoreBluetooth supports them — but **`btleplug` does not**, and `btleplug` is what the
laptop side would be written in. A transport the client cannot open is not a transport.

## What was measured

- **Negotiated ATT MTU with macOS: 251**, so **248 bytes are usable in one write** — not
  the 20 that the default MTU makes everyone design around.
- **The ceiling is the characteristic's storage on the device, not the MTU.** The GATT
  macro backs each characteristic with `[u8; T::MAX_SIZE]`. While that was 64 bytes, a
  65-byte write was refused with ATT **`Invalid Offset` (0x07)** — *refused*, not
  truncated. Loud is the failure mode worth having, and it is the one BLE gives.
- With the characteristic at 244 bytes: **238 bytes round-trip in a single write and a
  single notification**, byte for byte.
- **Image: 294 KB, 7.1 % of the 4 MB flash** — against 726 KB for the Wi-Fi build. BLE
  is a great deal cheaper in flash than Wi-Fi.
- **Heap: 46,140 of 73,728 bytes** after the host is built, and see below.

So a configuration blob of a few kilobytes is **ten to twenty frames**. No stream needs
building; a header (kind, frame *n* of *m*) with a CRC over the whole blob is enough.
And `write with response` is acknowledged at the ATT layer, so **ordered reliable
delivery is free** — no application-level ack, at the cost of one connection interval
per frame, which for twenty frames is invisible.

## The heap, measured — and why `alloc` is safe here

Asked directly 2026-08-25: the C3 has little RAM, so what stops it fragmenting?

`esp-alloc` is a thin wrapper over **`linked_list_allocator`** plus up to three
capability-tagged regions. One algorithm: an **address-ordered free list, first-fit**
(`allocate_first_fit`), with **immediate coalescing with both neighbours on free**
(`check_merge_bottom` / `check_merge_top` / `try_merge_next_n`). Every allocation runs
inside a `critical_section`.

What it does not have: compaction (impossible — the objects are behind raw pointers),
size classes, bins, slabs. **So fragmentation is real and the allocator will not save
you.** It also costs time, not just space: `allocate_first_fit` is O(number of holes)
with interrupts disabled, so a fragmented heap is a latency problem before it is a
memory problem — which matters more for a poll loop than for a web page.

The defence is architectural — **allocate at init, never in the steady loop** — and the
`internal-heap-stats` feature makes it *checkable* rather than merely asserted. The
metric is not `Current usage`; it is whether `Total allocated` climbs while the workload
is unchanged, because that is turnover, and only turnover can fragment. Measured on the
BLE firmware, three samples over thirty seconds of advertising:

```
after esp_wifi::init     Used  8 504 of 73 728
after BLE host built     Used 46 140            (packet pool, MTU 255 x 16)

steady state, unchanged across every sample:
  Current usage 46 192   Max usage 46 200
  Total allocated 46 300   Total freed 108
```

Turnover over the whole run: **108 bytes, once.** `current == max`. This firmware cannot
fragment, because after start-up it does not allocate.

Two places where that could stop being true, and they are the ones to watch:

1. **`format!` / `String` per request or per frame** — small short-lived allocations
   interleaved with long-lived ones, which is the textbook generator of holes. The Wi-Fi
   firmware writes its HTTP response through `core::fmt::Write` into a fixed slice for
   exactly this reason.
2. **Starting and stopping the radio repeatedly.** The C blob releases tens of kilobytes
   on stop and takes them again on start, into a different arrangement of holes. If `07`
   cycles the radio on a sleep schedule, this is the one place in the project where
   fragmentation can genuinely bite, and it wants a long soak, not an argument.

And one limitation to know: `esp-alloc` reports size, used and free, but **not the
largest free block**, so fragmentation is not directly observable — only inferable from
turnover, or discovered when a large allocation fails with plenty free.

## Landmines

Same shape as the Wi-Fi three in `10`: each one reports success.

1. **`esp-generate` produces a version-incompatible BLE project.** Its template pins
   `trouble-host 0.1.0`, which depends on `bt-hci 0.2.1`, while `esp-wifi 0.15.1`
   implements the traits of **`bt-hci 0.3.2`**. Both end up in the graph. The generated
   `main.rs` still **compiles**, because `ExternalController::new` has no trait bound and
   merely stores the transport; it would fail only when the controller is handed to the
   host. Use `trouble-host 0.2.x` with `bt-hci 0.3.2`. `embassy-sync` must also be a
   direct dependency — the `#[gatt_server]` macro names it.
2. **A characteristic's type must have a bounded size.** `&'static str` cannot be one:
   its `MAX_SIZE` is `usize::MAX`, and the macro's `[u8; T::MAX_SIZE]` backing array
   fails to lay out. The error reads "values of the type `[u8; usize::MAX]` are too big
   for the target architecture", which does not sound like "use `heapless::String`".
3. **A notification sent before anybody subscribes is dropped, by design.** The firmware
   sent a banner two seconds after connecting; the client was still reading Device
   Information and never saw it. Not a bug in either side — but a banner belongs on the
   subscribe (a CCCD write), not on a timer. This is the class of fault that looks like
   "works, except when you watch it".
4. **Host side: two `BufReader`s over one stdin eat each other's input.** The first
   buffers everything available and discards the remainder when dropped, so the second
   sees EOF. It does not reproduce interactively, only through a pipe. One reader per
   process. Recorded in `bleecho` itself.

## The client side

[`research/dash/bleecho`](../../research/dash/bleecho) — scan, pick a device from the
listing, echo over NUS. Written in Rust on **`btleplug`** (CoreBluetooth / BlueZ /
WinRT) rather than as a Python script, because it is the crate `vagcan` would use if BLE
becomes a transport, so what works there is reusable rather than merely indicative.

Deliberately **not a workspace member**: `btleplug` binds CoreBluetooth on macOS and
wants dbus/BlueZ headers on Linux, which would put both on the critical path of
`cargo build --workspace` and of CI for a tool the product does not link. The root
manifest excludes it; build it by manifest path.

**macOS never reveals a BLE device's address.** CoreBluetooth substitutes a UUID that is
generated per host and per peripheral (`c3204ad0-…` for this board on this Mac); Linux
and Windows give the real MAC. **Identify the device by name and service UUID**, never by
address, or the code works everywhere except on the machine it is being written on.

## What a configuration protocol still needs

- **Framing** over NUS: 244-byte frames, a header carrying kind and frame *n* of *m*, a
  CRC over the whole blob. Reliability and ordering come from `write with response`.
- **Persistence on the device** — **done 2026-08-25**, in [`12-settings.md`](12-settings.md):
  a `config` partition found by label at run time, two slots with a generation counter and
  a CRC, verified to survive both a reset and a firmware reflash.
- **No safety boundary is needed here**, and an earlier draft of this document wrongly
  claimed one. The catalogs are *flashed*, as a firmware built for one car — the board
  decodes nothing and has nowhere near the resources to. Configuration therefore selects
  **among what is already in the image**: which pages exist, what each shows, brightness
  and the like. A page cell is an index into the baked-in plan, so a forty-first
  identifier is not refused, it is unsayable. That is `README.md`'s "the device resolves
  nothing, it executes a plan" doing its job, and nothing needs to be added on top.

## Still unverified

- **The central role** — `scan` is built but not flashed, because the board can hold one
  firmware and `peri` is the one being used.
- **Bonding and encryption.** `trouble-host` has a `security` feature (LE Secure
  Connections, P-256, AES-CMAC). Untouched, and **not planned**: the configuration client
  is a program of ours — a TUI first, a phone application only if it turns out to be
  wanted — so there is no pairing dialogue in anyone's operating system to satisfy. What
  a parked car within ten metres of a stranger warrants is a separate question, and it is
  about the protocol rather than about bonding.
- **Power.** Nothing measured yet in any mode. `08` wants the whole device under about
  1 mA asleep, and the interesting comparison — Wi-Fi AP against BLE advertising against
  deep sleep — is exactly the argument for using BLE for configuration rather than
  raising an access point for it. Numbers, not adjectives, and none exist yet.

## Bench notes

The `10` notes all still apply — sandbox off for `/dev/cu.*`, `espflash monitor` resets
the board on attach, flash and monitor must not overlap. Two more:

- `espflash` needs `--port /dev/cu.usbmodem1101` explicitly when it cannot prompt; without
  a terminal it fails with `IO error: not a terminal`, which reads like a cable fault.
- macOS gates Bluetooth per application (TCC). A tool run from a terminal that has never
  been granted it will see **no adapter at all**, not an empty scan — the same shape of
  privacy restriction that made `networksetup` an unreliable oracle for Wi-Fi in `10`.
