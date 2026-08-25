# research / dash — the ESP32 board, from the laptop's side

Recon tooling for the dash subsystem (`todo/dash/`). What was proven on the
hardware, and the landmines found doing it, is in
[`todo/dash/10-c3-recon.md`](../../todo/dash/10-c3-recon.md).

## `bleecho`

Scans for BLE devices, lets you pick one from the listing, and drops into an
echo session over the Nordic UART Service: what you type is written to the
device, what it notifies back is printed.

```
cargo run --release --manifest-path research/dash/bleecho/Cargo.toml
```

It is **not a workspace member**, deliberately. `btleplug` binds CoreBluetooth
on macOS and needs dbus/BlueZ headers on Linux; as a member those would land on
the critical path of `cargo build --workspace` and of CI, for a tool the product
does not link. The root manifest lists it under `exclude`.

Two things it exists to answer, both of which it has:

- **can a laptop reach the board from Rust** — yes, and with the same crate
  `vagcan` would use if BLE ever becomes a transport;
- **how much fits in one exchange** — macOS negotiates ATT MTU 251, so 248
  bytes are usable per write. The real ceiling is the *characteristic's*
  storage on the device: exceed it and the server answers ATT `Invalid Offset`
  (0x07) rather than truncating, which is the failure mode you want.

It reads stdin once, in `main`, and passes the reader down. Two `BufReader`s
over one stdin silently eat each other's input — the first buffers everything
available and discards the remainder when dropped, so the second sees EOF. That
does not reproduce interactively, only through a pipe.

The firmware it talks to lives outside this repository for now (`~/esp/c3-recon/`),
because it is `no_std` on `riscv32imc-unknown-none-elf` with its own
`build-std` configuration and would not survive inside the workspace. `05`
already anticipates this: `vag-dash-fw` is "new, outside the workspace".
