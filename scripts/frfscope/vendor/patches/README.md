# Patches for the vendored VW_Flash

`scripts/vendor/VW_Flash/` is a local clone and is **not** committed, so any edit
to it would be lost on reinstall. Changes we depend on live here as patches and
are re-applied after cloning:

```bash
git clone --depth 1 https://github.com/bri3d/VW_Flash scripts/vendor/VW_Flash
cd scripts/vendor/VW_Flash
uv sync
for p in ../patches/*.patch; do git apply "$p"; done
```

## 0001-slcan-cross-platform-transport.patch

Adds an `SLCAN` interface so VW_Flash can talk CAN **on macOS** (and anywhere
else) through a serial slcan adapter, with no Linux kernel SocketCAN and no
J2534.

Why it is needed: `connection_setup()` only offered kernel SocketCAN
(`IsoTPSocketConnection`, Linux-only) or J2534 (`lib/connections/j2534.py` uses
`ctypes.WINFUNCTYPE`, i.e. Windows stdcall, plus a hardcoded `.dll` path — it
cannot even import on macOS). python-can and can-isotp were already dependencies
but unused by VW_Flash's own code.

What it does:

- `lib/connections/connection_setup.py`
  - new `SLCAN` branch building `can.Bus(interface="slcan", …)` →
    `isotp.CanStack` → `udsoncan.connections.PythonIsoTpConnection`, mirroring
    the SocketCAN branch's `tx_padding=0x55` and STmin.
  - new `enforce_min_separation(bus, sep_s)`: the SocketCAN path forces its
    transmit separation via the kernel's `tx_stmin`, but `can-isotp==1.9` has no
    `override_receiver_stmin`, so the floor is enforced by wrapping `bus.send`.
    It patches the **instance** (not a wrapper object) because `isotp.CanStack`
    type-checks for a real `BusABC`.
- `VW_Flash.py`
  - `SLCAN` added to `--interface` choices; new `--slcan_device`; name-mangled to
    `SLCAN_<device>` like the existing SocketCAN/USBISOTP handling; `SLCAN` is
    the default interface on `darwin`.

Usage:

```bash
python VW_Flash.py --interface SLCAN --slcan_device /dev/tty.usbmodem1101 \
  --action get_ecu_info
```

### Unit note (upstream comment is wrong)

`connection_setup()` is commented `# st_min is in us`, but the value is in
**nanoseconds**: `stmin_to_isotp()` divides by 1e6 to reach milliseconds and the
USBISOTP branch divides by 1e3 to reach microseconds. The default `350000` is
350 µs. The patch converts with `/ 1e9`; using the documented "µs" would have
made every frame gap 1000× too long (measured: 2650 ms instead of 3.15 ms for
10 frames).

### Caveat before trusting it with a write

Verified: the stack builds, the separation floor holds (10 frames @350 µs took
3.98 ms), and a bad device fails at port-open rather than on API misuse. **Not**
verified against a real ECU. Flash-rate ISO-TP timing is exactly what the
project validates on kernel SocketCAN, so prefer Linux (Pi or a VM) for an
actual **write**; use this path for reads on macOS.
