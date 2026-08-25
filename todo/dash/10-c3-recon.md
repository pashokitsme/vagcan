# dash / 10 — the board is an ESP32-C3, and what that changes

**Subsystem:** dash · **Needs the car:** no · **Recon done on hardware 2026-08-25**

Tasks `05`–`09` were written for an **ESP32-WROOM-32**. The board that is actually
plugged in is an **ESP32-C3 SuperMini**, and the difference is not cosmetic: it changes
the toolchain, deletes the mechanism `09` was built on, and weakens the argument `05`
used to choose its runtime. This document is what a day on the bench established, so
that the older tasks are read against measurements rather than assumptions.

## The board

```
espflash board-info
  Chip type:         esp32c3 (revision v1.1)
  Crystal frequency: 40 MHz
  Flash size:        4MB
  Features:          WiFi, BLE
  USB:               303a:1001 "USB JTAG/serial debug unit" (native, no bridge)
```

| | WROOM-32 (tasks `05`/`09`) | ESP32-C3 (on the bench) |
|---|---|---|
| core | Xtensa LX6 — needs the `esp` toolchain fork | **RISC-V — plain stable Rust**, `riscv32imc-unknown-none-elf` |
| USB | none; USB-C goes to a CP2102 | **native USB Serial/JTAG** — flashing *and* console, UART0 stays free |
| Bluetooth | Classic + BLE, so **SPP** | **BLE only. No SPP.** |
| GPIO | 34 | 22 — the pin table in `05` does not transfer |
| RAM / flash | 520 KB / — | 400 KB SRAM / 4 MB |

The board's blue LED sinks through **GPIO8** (Low is lit). GPIO8 and GPIO9 are strapping
pins on the C3, sampled at reset only; the LED's pull-up holds GPIO8 high there, so
driving it afterwards is safe. Same family of trap as `05`'s warning about `MTDI`/D12
on the WROOM-32 — a different pin, an identical way to lose a day.

## The stack: `no_std` / esp-hal, decided 2026-08-25

Two incompatible ecosystems exist, and the choice is the runtime, not a library:

- **`std` / `esp-idf-svc`** — Rust over ESP-IDF, the C framework: FreeRTOS threads, lwIP
  (TCP/IP with a DHCP **server**), NimBLE, `esp_http_server`, NVS. Real `std`.
- **`no_std` / esp-hal** — pure Rust: `embassy` instead of threads, `smoltcp` instead of
  lwIP, `esp-alloc` for the heap, `trouble-host` for BLE.

**`no_std` was chosen**, and the measured reason is build latency: **15 s to compile,
~1 s to flash**, against ESP-IDF's gigabyte of first-build download. The image is
**726 KB, 17.6 % of the 4 MB flash**, with the whole Wi-Fi stack and a web server in it.

`no_std` is **not** "no heap". `esp-alloc` registers a `#[global_allocator]`, so `String`,
`Vec`, `Box`, `BTreeMap` and `format!` are all available. What is lost is the *operating
system*: no `std::thread`, no `std::net`, no `std::fs`, no `HashMap` (needs OS entropy),
no unwinding. `core::error::Error` is stable, so error types are unaffected.

The heap has two faces, which is worth knowing before it confuses somebody: the Wi-Fi
blob is C and calls `malloc` by name, so `esp-wifi` supplies `#[no_mangle] extern "C" fn
malloc` that forwards to `esp_alloc::HEAP.alloc_caps(Internal, …)`. One heap, two
front doors. `heap_allocator!(size: 64 * 1024)` carries AP + DHCP + HTTP with room left.

## What runs on the board today

`~/esp/c3-recon/ap-web` (outside the repo; the checkout keeps no firmware):

- **`ap-web`** — open access point `vagcan-dash` on channel 6, static `192.168.71.1/24`,
  a DHCP server on UDP/67 handing out `.50`–`.200`, an HTTP server on TCP/80 returning a
  styled page, and a blue-LED heartbeat driven from the radio's own `ap_state()`.
  A phone joins it, gets an address and loads the page.
- **`scan`** — station-mode radio self-test. Proved the SuperMini's antenna: seven
  networks, the nearest at **−44 dBm**.

## Three landmines, all of them silent failures

Each cost real time, and each shares one shape: **the failure reported success.**

1. **`esp-wifi` cannot run a secured SoftAP.** Its README says so under *Missing / To be
   done: Support for non-open SoftAP*. Configure `AuthMethod::WPA2Personal` and
   `set_configuration` returns `Ok`, `start_async()` returns `Ok`, `ap_state()` reads
   `ApStarted` — and nothing beacons. **The AP has to be open**, so every refusal must
   live above the radio, on the device. `09` already required exactly that; it is now the
   only option rather than the preferred one.

2. **Never log from inside an `esp-wifi` event handler.** An `info!` in an
   `event::ApStadisconnected::update_handler` runs in the driver's own event context and
   writes to USB Serial/JTAG, which waits. That wedges the Wi-Fi task from within:
   beacons stop, while `ap_state()` still reads `ApStarted` and the firmware keeps
   printing that it is alive. A callback may set a flag or push to a channel — the
   printing belongs in a task. This one masqueraded as the WPA2 failure and cost an hour.

3. **`embassy-net 0.7.1` is not compatible with `esp-hal-embassy 0.9`.** The patch release
   moved from `embassy-time 0.4` to `0.5`, and `embassy-time` admits exactly one
   registered time driver. Two `Duration` types appear, and the version without a driver
   would have no clock. **Pin `embassy-net = "=0.7.0"`.**

And one of ours, which is a rule rather than a landmine: **never `.ok()` a `spawn`.**
The embassy task arena is finite and a full one fails silently, which looks exactly like
a task that started and did nothing.

## What this changes in the earlier tasks

- **`09` (the device as a wireless CANable) must be rewritten.** It rests on Bluetooth
  SPP presenting as `/dev/tty.*`, which the C3 cannot do. The replacement follows from
  `09`'s own argument: the slcan backend is *stream-generic*, so a **TCP socket over
  Wi-Fi** carries it unchanged — no custom GATT service and no per-platform client. The
  AP is open, so the allowlist enforced on the device is what makes this safe.
- **`05`'s runtime argument is weaker than it reads.** It chose ESP-IDF for real threads,
  so that `vag-transport`'s synchronous `IsoTpTransport` could be reused without a
  rewrite. But under embassy a blocking read stalls the single executor — it would stop
  the panel, the web server and BLE together. The device wants **async ISO-TP**;
  `uds_async.rs` already exists, `isotp.rs` does not have an async form yet. That is the
  real cost, and it is larger than "change some imports" and smaller than "rewrite the
  protocol".
- **`05`'s pin table is void.** The C3 has 22 GPIO, different strapping pins, and a
  different ADC layout. It has to be redone against the C3 before any wiring.
- **`07`/`08` are untouched in principle** — deep sleep, wake on the rail, and the
  quiescent-current budget are the same problems on either chip, with different numbers.

## `vag-protocol` and `vag-transport` on the device

Read on 2026-08-25, not yet compiled for the target — so this is a well-supported
expectation, not a result:

```
vag-transport/traits.rs    use std::time::Duration            → core::time::Duration
vag-transport/mock.rs      Duration, std::collections::VecDeque → core + alloc
vag-protocol/pdu.rs        use std::time::Duration            → core::time::Duration
vag-protocol/isotp.rs      use std::time::Duration            → core::time::Duration
vag-protocol/read.rs       use std::borrow::Cow               → alloc::borrow::Cow
vag-protocol/uds.rs        VecDeque, Duration                 → core + alloc
vag-protocol/address.rs    std::fs, std::env, RwLock          ← the one dirty file
```

No `std::io` anywhere in the transport seam — it is built on byte slices — and the only
`catch_unwind` in the workspace is in `vagcan/src/ui/term.rs`, which never ships.

`std` does not *define* `Duration`, `VecDeque` or `Cow`; it re-exports them. So the
rewrite is `use std::` → `use core::`/`use alloc::`, identical types on the host, no
feature flags needed. A flag is needed for exactly one thing:

```rust
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

#[cfg(feature = "std")]
pub mod address;
```

The device does not need `address.rs` in any case — the plan carries unit addresses.

## Bench notes

- Everything that touches `/dev/cu.*` needs the sandbox off.
- `espflash monitor` **resets the board on attach**, so every look at the log restarts the
  AP. `--no-reset` is not the fix: it connects through the flash stub and leaves the chip
  in the bootloader with the application stopped.
- Flash and monitor must not overlap; the first still holds the port when the second opens.

## Next

**BLE.** The C3 has it, `trouble-host` is the `no_std` host stack, and `esp-generate`
offers `-o ble-trouble`. What it is *for* is a separate question from whether it works:
with slcan moving to TCP, BLE's role is no longer the wireless-adapter path.
