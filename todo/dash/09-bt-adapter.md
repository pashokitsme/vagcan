# dash / 09 — the device is also a wireless CANable

**Subsystem:** dash · **Crate:** `vag-dash-fw`, `vag-uds-can` · **Needs the car:** no

> **The board changed, 2026-08-25.** The bench board is an **ESP32-C3 SuperMini**, not the
> WROOM-32 this document was written against: RISC-V on stable Rust, native USB, **BLE only
> — no Bluetooth Classic, no SPP**, 22 GPIO. What was proven on hardware, and what it voids
> here, is in [`10-c3-recon.md`](10-c3-recon.md). Read that first — and
> [`11-ble.md`](11-ble.md) for what BLE does carry, which is configuration rather than a
> bus: it cannot move a loaded CAN bus, and the measurement is there.

## Goal

A laptop or a phone pairs with the device over Bluetooth and gets a serial port that
speaks **slcan**. To `vagcan` it is then indistinguishable from the cable, except that
there is no cable.

The owner asked for this on 2026-08-20 and it turns out to be nearly free, for two
reasons that only became visible once the hardware was on the table.

## Why it is nearly free

**The board is the classic ESP32, so Bluetooth Classic and SPP are available.** This is
the one place where the older part beats the newer: the S3 and C3 are BLE-only, and BLE
has no serial port profile — it would need a custom GATT service and a custom client on
every platform. SPP presents as `/dev/tty.*` and is opened like any other port.

**And `vag-uds-can`'s slcan backend is already stream-generic.** Its own manifest says so: the
codec and the backend build and are tested without `tokio-serial`, which is only there for
the constructor that opens a real port. Over an SPP socket it is the same code, unchanged.

So the work is a `slcan` *server* on the device — the mirror of the client we already have
— speaking the same ASCII protocol over an SPP link, backed by TWAI.

## What it must not become

**Read-only, like everything else here.** The slcan protocol can transmit arbitrary
frames, and a server that accepted any frame from a paired phone would be a write path
into the car with a Bluetooth radio in front of it. The allowlist is `0x22`, `0x19`,
`0x10`, `0x3E`, it applies to what arrives over the air exactly as it applies to what the
plan asks for, and the refusal happens **on the device** — never in the client, which is
not the thing holding the transceiver.

And pairing is a pairing, not an authorisation: anything that could change how a unit
behaves stays refused regardless of who is connected.

## Interaction with the rest

- The panel keeps rendering while somebody is connected. The polled set becomes the union
  of the page, the armed alarms and whatever the client asks for — the same union rule as
  `04`, with a third contributor.
- A connected client is a reason **not** to sleep (`07`).
- Bluetooth is not free: the radio is milliamps, and it belongs off unless somebody is
  connected or asking. Pairing is a deliberate act, so the default is off.

## Tests

- The slcan server round-trips against `vag-uds-can`'s own codec — the client and the server
  are two ends of one protocol and each is the other's best test.
- A frame arriving over the air whose service is outside the allowlist is refused, and the
  refusal is asserted on the device side.
- The union rule holds with a client attached.

## Done when

`vagcan` on the laptop opens the paired serial port and reads the car through the device,
with the panel still updating.
