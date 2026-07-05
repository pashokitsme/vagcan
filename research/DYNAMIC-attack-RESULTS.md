# Dynamic attack — RESULTS (offline analysis of the 5 live VCDS-RUS memory dumps)

Offline analysis of `research/dumps/{VCDS,VCDS-2,VCDS-3,VCDS-4,VCDS-after-scan}.exe.dmp`
(Windows minidumps, x86 process under ARM WOW64, one auto-scan session on the owner's
own Škoda, VIN `XW8AD4NE9JH008917`). Goal was to recover the OLD-scheme per-epoch AES
session key `K_epoch` and reverse the app-side KDF. See `DYNAMIC-attack-playbook.md`
for the plan this executed against.

Tooling added (all in `research/clb-crack/`, run with `.venv/bin/python`):
`aes_scan_fast.py` (vectorised AES-128/192/256 schedule scanner),
`scan_dump_keys.py` (minidump-region-aware scanner with VA/module mapping),
`validate_k.py` (K → per-channel keystream → UDS decode validator).

## TL;DR — the memory-scan path does NOT work for this build

1. **No AES key schedule of ANY size (128/192/256), in either standard or
   LibTomCrypt word-swapped byte order, exists in ANY of the 5 dumps.** The scanner
   is verifiable (a 240/208/176-byte window either IS a valid FIPS-197 expansion or
   is not) and passes synthetic self-tests for all three sizes and both layouts.
   Result across all 5 dumps (full 429–436 MB each, standard + word-swapped layout):
   **0 keys.** The fast scanner's entropy pre-filter (`len(set(key)) >= 12` for
   AES-256, 10/8 for 192/128) cannot reject a real key — a random/KDF-derived 32-byte
   key has ≈30 distinct byte values, so P(<12 distinct) is astronomically small — so
   the 0-result is conclusive, not a filter artefact.
2. **No XOR-valid `b8`/`b7` link frames exist in any dump** either (strict
   `53 14 b8 <16> xor` / `4d 14 b7 <16> xor` checksum validation → 0 hits). The
   raw wire frames are consumed and discarded; the framed ciphertext is not retained.
3. The dumps hold only the **high-level decoded application data** — the VIN and
   every part-number are present as length-prefixed Delphi strings (e.g. VIN as
   `11 00 00 00` + `XW8AD4NE9JH008917`), ASCII + UTF-16LE, in all 5 dumps. The
   link-layer 16-byte plaintext blocks and the raw UDS PDUs (`62 f1 90 …`) are NOT
   adjacent — the wire data is fully lifted into app strings before the dump moment.

∴ The playbook's central assumption ("the expanded AES schedule sits in cleartext in
the cipher context, scan finds `K`") is **false for this build.** The link cipher is a
custom/table AES statically linked *inside* the VMProtect-packed VCDS.exe; its round
keys are never materialised as 240 contiguous cleartext bytes on a normal heap (either
recomputed per-frame in VM context, or held in a VMProtect-managed region). `K_epoch`
remains sealed. The KDF-reversal path is unchanged: still blocked by VMProtect.

## The `vcds_hook.dll` lead — characterised, and it is NOT a shortcut

The crack (VCDS-RUS) injects `vcds_hook.dll` (VA `0x1a730000`, ~1.19 MB, present in
every dump). Carved it from the mid-scan dump (all 291 pages present; MZ/PE intact).
**In-memory it is unpacked** (code section entropy 6.61; the on-disk copy is
VMProtect-packed, section "1111" entropy 7.92, code section rawsize 0). Readable copy
analysed:

- **It is "Hook.32.dll" — a transparent FTDI/FTD2XX proxy shim.** Exports the entire
  87-function `FT_*` surface (FT_Open/Read/Write/ListDevices/SetBitMode/…) plus two
  Delphi exports `TMethodImplementationIntercept` + `dbkFCallWrapperAddr`. Built with
  **Delphi + DDetours** (Cheat-Engine DBK) — string `DDetours.TThreadsIDList` confirms
  inline in-process detours. It sits between VCDS.exe and the real RT-USB.dll,
  forwarding `FT_*` and detour-patching VCDS methods (the license/genuineness bypass
  that lets a clone/genuine cable run under the cracked build).
- **The shim contains NO cryptography.** No AES S-box, no AES T-tables, no SHA-1/256
  init or round constants, no `RCON` — searched the full unpacked image and found none.
  No `bcrypt`/CNG/CryptoAPI imports (imports are only advapi32/kernel32/netapi32/
  oleaut32/user32/version; delay-imports are 5 unrelated OS functions). The exported
  `FT_*` handlers are `ret`-stubs / a pointer table (detour trampolines), not crypto.

**Answers to the hook questions:** (1) FT_Read/FT_Write do not compute or inject
`K_epoch` — the shim has no crypto to do so; any wire-level tampering it does is
genuineness/license spoofing, not key derivation. (2) It detours in-process VCDS
methods via DDetours, but since the shim carries no crypto those targets are the
license/genuineness patch, not the link-key path. (3) The shim calls no bcrypt/AES;
it only forwards `FT_*` to the real ftd2xx. **The link key stays entirely inside
VCDS.exe. The hook does NOT shortcut the KDF.**

## Positive locations recovered (for the KDF, if VCDS.exe is ever unpacked)

The OLD x86 VCDS.exe embeds the SAME static tables as the analysable ARM64 build:

| item | ARM64 VA | found in VCDS-2 dump at | live VA / module |
|------|----------|--------------------------|------------------|
| 16-row link-cipher IV_TABLE | `0x140171d30` | file off `0x271646` (16 contiguous rows, byte-identical) | `0x5384d0` [VCDS.exe] |
| 128-byte genuineness/static table | `0x140171730` | file off `0x2ad62e` (byte-identical) | `0x5744b8` [VCDS.exe] |

So the KDF's static-table input is confirmed present and identical to the ARM64 build's
table (VA `0x5744b8` in the live x86 image). What is still missing to build the KDF
tuple `(K, static_table, counter, b6-nonce)` is `K` itself (not in memory, per above)
and the counter/nonce loci (not pursued once `K` proved unrecoverable — without `K`
there is nothing to hypothesis-test against).

## Cross-dump summary

| dump | when | AES keys (any size) | XOR-valid b8/b7 | decoded VIN/part-# present |
|------|------|---------------------|-----------------|----------------------------|
| VCDS         | mid-scan  | 0 | 0 | yes |
| VCDS-2       | mid-scan  | 0 | 0 | yes |
| VCDS-3       | mid-scan  | 0 | 0 | yes |
| VCDS-4       | mid-scan  | 0 | 0 | yes |
| VCDS-after-scan | post-scan | 0 | 0 | yes |

## Recommendation (what's left)

The three paths from `RE-PLAN-old-scheme-rekey.md` UPDATE 2 stand, minus the
now-closed memory-scan route:

1. **VMProtect dynamic unpacking of VCDS.exe** to recover the custom-AES schedule
   computation / the symmetric KDF. This is now the ONLY route to `K` (the round keys
   are not exposed in a static memory image). Requires emulation/dynamic instrumentation
   inside the running x86 process (e.g. hardware breakpoint on the AES-encrypt block
   fed the IV rows, dump the round keys from registers/stack at that instant) — a live
   debugger/instrumentation session, not an offline dump.
2. **Exact-complete 1:1 replay from fresh power-on** [CABLE] — unchanged.
3. **Generic USB-CAN bypass** (`vag-can` + a slcan dongle) — the pragmatic route to
   `vagcan info`; sidesteps the clone link entirely.

Since an offline memory dump cannot yield `K` for this build, the cheapest next step to
*decode this session* is instrumentation (breakpoint the AES block in the live process
and read the round keys); the cheapest route to the *product goal* remains generic CAN.
