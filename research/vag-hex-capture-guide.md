# `vag-hex` — USB Capture Guide

> **DONE (2026-07-05).** Two captures were taken — `research/init-only.pcapng`
> and `research/reading-ecus.pcapng` — and fully analyzed. Wire format recovered
> and implemented (`crates/vag-hex/src/frame.rs`), link cipher reversed
> (`research/clb-crack/link_cipher.py`). See `research/vag-hex-framing.md` for the
> results. This guide is retained as the method record; the sections below
> describe how the captures were produced.

**Purpose.** Record the clone HEX cable (VAG25.3) talking to a *working* VCDS install, so we
can model the cable's own USB/serial protocol and drive it directly from `vagcan` — no VCDS,
no loader. This capture is the single input gating P1. Everything below runs on the Windows
box where VCDS already works with the cable.

---

## 0. What we're trying to learn

From one good trace we need to recover four things:

1. **Enumeration** — how the cable presents on USB (FTDI VID/PID, is it D2XX or a virtual COM
   port, bcdDevice, iProduct string).
2. **Init handshake** — the fixed byte exchange VCDS does right after opening the cable, before
   any car traffic (baud/latency setup, firmware/version query, "hello").
3. **Framing** — how a UDS request (e.g. `22 F1 90` read VIN) is wrapped for the wire:
   ISO-TP over the cable's own serial envelope, with whatever length/checksum bytes the cable adds.
4. **Request↔response pairing** — matched pairs so we can prove the framing both directions.

---

## 1. Tools (Windows)

- **Wireshark** (latest) — <https://www.wireshark.org/download.html>
- **USBPcap** — the USB capture driver. Ships with the Wireshark installer; make sure the
  "USBPcap" checkbox is ticked during install. Reboot after installing if prompted.
- (Optional CLI alternative) **USBPcapCMD.exe**, installed under `C:\Program Files\USBPcap\`.

---

## 2. Identify the cable's USB bus

1. Plug the cable in. Let VCDS's driver bind (the working setup).
2. Open **Device Manager** → find the cable. It'll be under **Ports (COM & LPT)** as a
   USB Serial Port (virtual COM), or under **Universal Serial Bus controllers** as an FTDI
   device. Note:
   - which it is (COM port vs raw USB device),
   - the **VID/PID** (Properties → Details → *Hardware Ids*, looks like `USB\VID_0403&PID_6001`),
   - the **COM number** if it's a COM port (e.g. COM3).
3. In Wireshark's capture list you'll see **USBPcap1, USBPcap2, …** — one per host controller.
   The cable lives on one of them. If unsure, capture on the one whose device tree (shown when
   you hover / in USBPcapCMD's device list) contains your FTDI VID/PID.

> Write the VID/PID and COM number into the deliverable notes (§6). They're half the answer to
> "how does it enumerate."

---

## 3. Capture procedure

Goal: a clean, *short*, well-labelled trace. Short is better than long — we want isolated,
identifiable exchanges, not a 10-minute firehose.

1. **Close VCDS.** Start the Wireshark capture on the correct USBPcap interface **first**, with
   the cable already plugged in.
2. **Launch VCDS.** This records the open + init handshake (§0.2) from the very first byte —
   critical, don't skip. Wait for VCDS to show the cable as ready / "test" OK.
3. In VCDS, do these actions **slowly, one at a time**, pausing ~2 s between each so they're
   visually separable in the trace:
   - **Interface test / "Test"** (Options → Test, if present) — pure cable handshake, no car.
   - **Auto-scan** *or* just **Select → Engine (01)** — first car conversation.
   - **Read the VIN** (measuring/ident block that shows the VIN) — known plaintext `22 F1 90`.
   - **Open Measuring Blocks**, watch **one** group for ~5 s (e.g. RPM/coolant) — repeated
     `22 <did>` polls, gold for framing.
   - **Read fault codes** (DTCs) on the engine — a `19 02 …` exchange.
   - **Clear nothing.** Read-only actions only.
4. **Stop VCDS**, then **stop the capture.**
5. **Save as `.pcapng`** (File → Save As). Name it `vcds-cable-<date>.pcapng`.

If the cable is a **virtual COM port** and USBPcap traffic looks opaque/bulk-only, also grab a
**serial-level** view if easy (e.g. a COM port sniffer), but USBPcap alone is usually enough —
FTDI bulk-IN/OUT transfers carry the serial bytes directly.

---

## 4. Trim / annotate (optional but helpful)

In Wireshark, apply a display filter to keep only the cable's device once you know its address:

```
usb.device_address == <N>
```

(find `<N>` in any packet's USB layer). File → Export Specified Packets → save the filtered set.
Even better: note the frame numbers where each §3 action starts (e.g. "VIN read ≈ frame 812").

---

## 5. Sanity self-check before sending

- Trace starts **before** VCDS launched (captures the open handshake)? ✔
- You can see a burst of small OUT/IN transfers right after launch (init)? ✔
- At least one exchange you can label as VIN / measuring / DTC? ✔
- File is `.pcapng`, opens cleanly in Wireshark? ✔

---

## 6. What to hand back

Drop into `research/`:

1. `vcds-cable-<date>.pcapng` — the trace.
2. A few lines of notes:
   - cable VID/PID, and COM port vs raw USB,
   - VCDS version used for the capture,
   - rough frame numbers (or timestamps) for: init done, VIN read, measuring poll, DTC read.

That's everything. From it I model the enumeration, handshake, and framing, and build the
`vag-hex` crate against real request/response pairs.

---

## 7. Notes / gotchas

- **Use the OLD, working VCDS** for this — the one your cable already runs with. Version
  doesn't matter; we only need the cable's wire behavior, which the cable defines, not VCDS.
- USBPcap captures **all** devices on a host controller. If the trace is noisy (keyboard/mouse
  on the same controller), filter by `usb.device_address` (§4) — or plug the cable into a USB
  port on a different controller from your input devices.
- FTDI transfers: the first 2 bytes of each FTDI bulk-IN packet are **modem/line status**, not
  payload — I'll strip those during analysis, but don't be surprised to see them.
- Keep it read-only. Nothing here writes to the car; measuring blocks and DTC reads are safe.
