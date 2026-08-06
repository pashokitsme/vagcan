# Simos18.1 FRF → calibration pipeline (offline study + flash plan)

**Status:** offline extraction pipeline working and reproducible on macOS M4.
Flashing is *not* done here — see the safety boundary below.

## Safety boundary (read this first)

`vagcan` stays **read-only**. It never flashes, unlocks, codes, or adapts —
that line does not move (see [`../SAFETY.md`](../SAFETY.md), [`../CLAUDE.md`](../CLAUDE.md)).
Everything in this document that *writes* to an ECU is done by the **separate,
external** tool `bri3d/VW_Flash`, run by the owner on their own car. This file is
a research writeup of that external workflow plus our offline analysis of a
publicly-hosted factory flash container. No write capability is added to
`vagcan` by any of this.

The car whose firmware this concerns is the reference Škoda Octavia III (MQB,
1.8 TSI CJSA, engine ECU Simos 18.1, part `06K907425B`).

## The firmware under study

Source file: `research/FL_06K907425B__0004.frf` (factory flash container,
publicly hosted, e.g. mib.plain.gg).

| Field | Value |
|---|---|
| ECU spare-part number (F187) | `06K907425B` |
| ODX / flash container ident (F19E) | `EV_ECM20TFS02106K907425B_001` |
| Flash-container version | `0004` |
| Software family | `SC8` = Simos 18.1; this cal is sub-type **`SC8O10`** (`CASC8O10.DAT`) |
| Per-block software versions | CBOOT `0023`, ASW `0054`, CAL `0172` |
| Blocks | CBOOT, ASW1, ASW2, ASW3, CAL (5) |

### Provenance (SHA-256) — re-run and compare to detect corruption/drift

```
ce08170da0ce05dc4ae0ba7bb9292d6a5e52c104fa89c991e8740431b52db8c7  FL_06K907425B__0004.frf
9c87c7e4888f5bce4602f9fe971f290849da030761dd6a2bcbf1f91e46c68703  FL_06K907425B__0004.odx  (decrypted container)
7592bfaae5dc690563b86616a96e4f1543e3fb853599765ed17d1738ab0abab9  FD_0.CBOOT.bin
336c92114e01731dfa89c852f0af6a1b26dfe0a3f92b343057d7d764bf7dce42  FD_1.ASW1.bin
2cab57585c05e490db671754c419fe90feb3254d3f16c332c237e95168627d68  FD_2.ASW2.bin
6658e11e4e4dea0d83c7a2b47afcac6d9c6ff756d21edd0f1c60f95c635de913  FD_3.ASW3.bin
9f682ca3aaa9b149e0ec2c83d6a73924af0bb29f31cde5936e9f8cdd31f17029  FD_4.CAL.bin
```

The extraction is a pure deterministic transform (XOR-cipher decrypt → unzip →
block split), so these hashes are stable across re-runs. A mismatch means either
a different input file or a changed VW_Flash version.

## The offline extraction pipeline (reproducible, no car, no patch)

Tooling: `bri3d/VW_Flash` cloned locally, driven by `uv` (Python 3.14). The FRF
decryption key ships in the repo (`data/frf.key`); nothing external is needed.

```bash
# 1. clone
git clone https://github.com/bri3d/VW_Flash
cd VW_Flash

# 2. decrypt+unzip the FRF into its ODX container
uv run python -m frf.decryptfrf \
  --file /path/to/FL_06K907425B__0004.frf \
  --outdir out            # → out/FL_06K907425B__0004.odx

# 3. split the container into raw flash blocks (fully offline)
uv run python VW_Flash.py --action prepare \
  --frf /path/to/FL_06K907425B__0004.frf
#   → FD_0.CBOOT.bin FD_1.ASW1.bin FD_2.ASW2.bin FD_3.ASW3.bin FD_4.CAL.bin
```

`FD_4.CAL.bin` (511 KiB) is the **calibration** — the tuning surface. It is
stored **plaintext/structured** (entropy 6.13 bits/byte, 26 % zero bytes; a
cipher would sit near 8.0), so its maps can be read directly with a definition.

### macOS reality check

- **Extraction works natively on macOS M4** — steps 1–3 above run to completion.
- The `prepare` step ends by *importing* the flash transport, which pulls
  `ctypes.WINFUNCTYPE` (Windows stdcall) and raises
  `cannot import name 'WINFUNCTYPE' from 'ctypes'`. **This happens after the
  block files are already written** — extraction is unaffected. It only means the
  J2534 flash path cannot even load on macOS (see flashing section).

## This FRF is almost certainly NOT what the car runs

`scripts/vendor/VW_Flash/data/box_codes.csv` carries the exact row:

```
box_code,  version, engine_name, cboot,    asw,      cal,      ecm3_start, ecm3_end
06K907425B,0004,    ECU raw,     SC8E0L20, SC8E0O10, SC800O10, 55112,      65332
```

**`ECU raw` is the only such engine_name in all 435 rows** — every other row names
a real engine (`1.8l R4 TFSI`, `2.0l R4 TFSI`, …). Together with the container
being keyed on the bare *hardware* part number (`06K907425B`) rather than a
vehicle *system* part number (`5E0906264…`, `8V0906264…`, `3G0906264…`), the
strong reading is that **`SC800O10` is the blank service software shipped on a
new spare ECU, not a calibration any car runs in the field.**

What real 1.8 TFSI cars on this ASW branch (`SC8E0O…`) actually run:

| Calibration | ECM3 start | Example system part numbers (1.8 l) |
|---|---|---|
| `SC800O30` | 55328 | `5TD906264`, `5NG906264`, `8VD906264A` |
| `SC800O40` | 55360 | `3G0906264`, `3VD906264`, `8V0906264H`, `8V0906264J` |

So the car's real target is very likely **O30 or O40**. Read the car before
treating this file as a baseline — if F187/F19E report a system part number, this
FRF is a reference image only, and the definition hunt retargets accordingly
(commercial damos packs do exist for O20/O30/O40).

## Is this EXACTLY the car's firmware? — verification procedure

This FRF is *a* known-good stock `06K907425B_0004`. Whether it is **exactly what
the car currently runs** is only knowable by reading the car. Do this before
treating `0004` as "the car's stock":

```
vagcan info                 # identity sanity
vagcan properties 01        # engine ECU: F187 (part no), F189 (SW version), F19E (ODX)
```

Match criteria:

| DID | Car must report | From this FRF |
|---|---|---|
| F187 | `06K907425B` | `06K907425B` |
| F19E | `EV_ECM20TFS02106K907425B_001` | `EV_ECM20TFS02106K907425B_001` |
| F189 | the SW version string | container `0004`, CAL block `0172` |

- **F187 + F19E match** → this is the right ECU/software *line*. They match any
  `06K907425B` on this software, so they prove family, not revision.
- **F189 is the decisive check.** If the car's F189 corresponds to container
  `0004` / CAL `0172`, this FRF is the car's current stock. If it differs, the
  car runs a different revision and this file is a *reference base*, not the
  car's stock.
- **The only true stock backup is a full read off the car itself**, not this FRF.
  A downloaded FRF is a clean factory reference; it is not proof of, nor a
  substitute for, the car's own current bytes. Read and archive the car's own
  flash before writing anything.

## Flashing plan (external VW_Flash — not vagcan)

macOS cannot flash: the J2534 layer is Windows-only (`WINFUNCTYPE`, hardcoded
`.dll` path in `lib/constants.py`), and no macOS-ARM J2534 dylib for the VNCI
cable is known to exist. Two viable paths:

1. **Linux (Raspberry Pi or UTM VM) + SocketCAN + the slcan adapter** —
   recommended. Uses VW_Flash exactly as shipped; kernel ISO-TP timing is what
   the project validates against.
   ```
   sudo slcand -o -c -s6 /dev/ttyACM0 can0   # s6 = 500 kbit/s
   sudo ip link set can0 up
   uv run python VW_Flash.py --interface SocketCAN --can_channel can0 <action>
   ```
2. **Native macOS via the slcan patch — implemented**, see
   `scripts/vendor/patches/0001-slcan-cross-platform-transport.patch`. Adds an
   `SLCAN` interface (`can.Bus(interface="slcan")` → `isotp.CanStack` →
   `PythonIsoTpConnection`) plus a frame-level STmin floor, since
   `can-isotp==1.9` has no `override_receiver_stmin` to match the kernel's
   `tx_stmin`.
   ```
   python VW_Flash.py --interface SLCAN --slcan_device /dev/tty.usbmodem1101 \
     --action get_ecu_info
   ```
   Verified offline only (stack builds; separation floor holds — 10 frames at
   350 µs took 3.98 ms; a bad device fails at port-open). **Never tested against
   an ECU.** Use it for reads on macOS; prefer path 1 for an actual write, where
   flash-rate ISO-TP timing is what upstream validates.

Write sequence (both paths), from VW_Flash docs:
```
# once: install sample-mode CBOOT (CRC kept, RSA signature check disabled)
uv run python VW_Flash.py --action flash_unlock --frf <unlocker>.frf
uv run python VW_Flash.py --action get_ecu_info   # Hardware Version H13 → X13 confirms unlock

# flash a full stock/base FRF, CBOOT auto-patched to sample mode
uv run python VW_Flash.py --action flash_frf --frf FL_06K907425B__0004.frf --patch-cboot

# later, calibration-only writes (checksums+ECM3+compress+encrypt automatic)
uv run python VW_Flash.py --action flash_cal --infile FD_4.CAL.bin --block CAL
```

Recovery:
- **Soft (in car):** sample-mode CBOOT keeps CRC checking; an aborted/failed
  write leaves the ECU in programming mode awaiting a valid file — re-flash.
- **Hard (dead CBOOT):** bench only — `bri3d/TC1791_CAN_BSL` +
  `bri3d/Simos18_SBOOT`, ECU removed and opened. Keep a Pi + CAN-FD hat as the
  recovery rig.

## Definition status (for later, when tuning starts)

- No public A2L/XDF exactly matches `SC8O10` / `06K907425B_0004`.
- Closest public definition: `joeFischetti/SimosDefinitions` →
  `SC8S50_8V0906259K.xdf` (143 named tables: torque limiters, boost/MAP limits,
  rev limiter, PE lambda, spark…). It is an **`SC8S50`** definition.
- **S50 addresses align with our `SC8O10` binary for MOST maps, but not the
  torque structure.** A full sweep of all 143 S50 tables over the O10 CAL,
  scored by flatness + smoothness (`scripts/frfscope/frfscope.py --report`, or
  the browser view's flat/noisy filters): **93 structured & plausible, 41 flat,
  9 noisy/garbage.** The plausible set decodes to
  real values — Rail Pressure `ip_fup_sp_bas_sel[*]` 500–20000, Base Fuel MPI
  bottoming at 14.7 (stoich AFR), burble ignition `ip_iga_imp_comb_*` −24…0°,
  ethanol 0–100 %. The calibration layout is largely **shared** across SC8
  sub-versions (same ASW major).
- **The exception is exactly what we'd tune first.** `Max Reference Indicated
  Torque` (z-grid `0x69D3E`) reads `1798, 2312, 1799…` → ~900 N·m with a
  gear axis in the tens = garbage; the torque limiters do not sit at the S50
  offsets on O10. So: fuel/spark/rail maps are mostly usable from the S50 def as
  a template; **torque-structure maps must be re-based and every map verified by
  physical sanity** before it is trusted. Nonsense in `frfscope` (900 N·m, gear
  50, lambda 30) is the tell for a wrong address.
- **Negative result — do not retry: auto-re-basing maps by structural search.**
  The idea was, for a map whose S50 address decodes to garbage, to scan the whole
  CAL for an offset whose R×C grid is "map-like" (non-flat, smooth, monotonic
  along rows or columns) and propose it as the true O10 address. Measured on the
  real binary (`scratchpad/rebase_probe.py`), the criteria match **5 637–49 412
  offsets per map** (`Max Torque at Clutch MT` 7×12 → 14 210; `Rev Limiter` 8×3 →
  49 412). A 511 KiB calibration is dense with map-like data, so structural
  criteria alone carry no discriminating power. Any tool built on this would emit
  noise dressed as answers. Re-basing needs a real anchor — a correct A2L, or
  manual reversing of the code that reads the map.
- **Corollary: the value gate cannot certify.** The same torque maps that decode
  to ~900 N·m are classified `ok` by the structural gate. `ok` means "the bytes
  here look like a map", never "this is the right map". Only an exact-version A2L
  or hardware validation settles it.
- Correct approach: use the S50 XDF as a **name/structure/scaling template** and
  **re-base each map** onto our CAL by matching axis signatures + dimensions,
  validating with physical sanity. Tool: `jtownson/xdfbinext`. A true `SC8O10`
  A2L (paywalled/leaked on MHH/ecuedit) would remove the guessing.

## Open items

- [ ] **Read car F187/F189/F19E** — decides everything downstream. Expect a
      *system* part number and an O30/O40 calibration, not this `06K907425B`
      service image.
- [ ] Re-target the definition hunt to whatever the car actually reports.
- [ ] Full stock read off the car → archive as the real backup (two locations).
- [ ] Decide flash host: Pi / UTM Linux VM (writes) vs the macOS slcan patch
      (reads).
- [x] ~~Auto-re-base maps by structural search~~ — refuted, see negative result.
