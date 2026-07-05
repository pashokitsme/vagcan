# VCDS-RUS crack + live-session artifacts — findings

Session 2026-07-05/06. The owner runs **VCDS-RUS 24.7.1.0** (cracked Russian build,
data 20240617 DS356.3) in an ARM Win11 VM (x86-on-ARM WOW64, `xtajit.dll`), cable
passed through. Full Auto-Scan succeeded → gave us ground-truth vehicle data AND
5 process memory dumps + the crack's dropped files. This is the dynamic-attack material
for recovering the OLD-scheme link key `K_epoch` (see `DYNAMIC-attack-playbook.md`).

## Ground truth from the Auto-Scan (validation oracle)

- **VIN: `XW8AD4NE9JH008917`**, chassis **NE-SK37 (3Q0)** = Škoda, mileage 212140 km.
- Scanned addresses: 01 02 03 08 09 10 15 16 17 19 42 44 52 5F 75.
- Key ECUs (addr → part / component / VCID):
  - 01 Engine `J623-CJSA`: SW `8V0 906 264 H`, HW `06K 907 425 B`, `1.8l R4 TFSI`,
    VCID `418318C6D089C39E84-8015`. Fault 15187 (P0850). label `06K-907-425-V1.clb`.
  - 02 Auto-gearbox `J743` DQ200: SW `0CW 300 041 G`, comp `DQ200G2` `1003`.
  - 03 ABS `J104`: `5Q0 614 517 AQ`. 08 Climatronic `5E0 907 044 AM`.
  - 09 BCM `J519`: `5Q0 937 084 CF`. 19 Gateway `J533`: `3Q0 907 530 B` (holds VIN).
  - 17 Kombi `5E0 920 740 D`, 5F MIB `5E0 035 871 C`, 15 Airbag `3Q0 959 655 BE`, …
- Any dump buffer / decoded b7 that yields these ASCII/UDS bytes CONFIRMS a recovered K.

## Artifacts (in `research/dumps/`, gitignored — proprietary/PII)

- `VCDS.exe.dmp`, `VCDS-2/-3/-4.exe.dmp` — minidumps at random moments DURING the scan
  (link keyed, live AES round keys in memory). ~430 MB each.
- `VCDS-after-scan.exe.dmp` — after the scan finished (keys may be cleared).
- `vcds_hook.dll` (292 KB, x86 PE) and `VCDSLoader64.exe` (1.6 MB, x86-64) — the crack's
  files, dropped by the loader into Temp.

## The crack architecture (VCDS-RUS)

- **VCDSLoader64.exe injects `vcds_hook.dll` into VCDS.exe** — VCDS starts as a child of
  the loader, then detaches (owner-observed). Hook loads at VA **0x1a730000** (~1.14 MB
  in-memory; the on-disk drop is 292 KB packed).
- **`vcds_hook.dll` = FTD2XX PROXY SHIM + Delphi detour hook:**
  - Exports the **full FT_\* surface** (89 exports: FT_Open/Read/Write/Close/ClrDtr/
    SetBitMode/…). It **replaces RT-USB/FTD2XX** — sits on the wire between VCDS and the
    real cable driver, seeing every FT_Read/FT_Write byte.
  - Non-FT exports `TMethodImplementationIntercept` + `dbkFCallWrapperAddr` → built with
    **Delphi + Cheat-Engine DBK / Delphi-Detours**. So it also **detour-hooks VCDS methods
    in-process** (license/genuineness patch, possibly the link crypto).
  - On-disk copy is **VMProtect-packed** (section `1111` entropy 7.92, `0000` bss 0xdb000;
    same packer as VCDSLoader). Readable: export table + import table only
    (advapi32 RegCloseKey, KERNEL32 LoadLibraryA/GetProcAddress/VirtualProtect, netapi32
    NetWkstaGetInfo, oleaut32 VariantCopy, user32 IsWindow, version VerQueryValueW).
  - **Analyze the UNPACKED copy carved from a mid-scan dump at 0x1a730000**, not the packed
    on-disk drop.

## Open questions the dumps should answer

1. **Does the shim intercept/modify FT_Read/FT_Write?** If it spoofs the cable's
   genuineness/identify response or injects/alters the b6/b7 or the session key so ANY
   cable passes, that logic is the prize (the crack making the clone work). If it just
   forwards FT_* + patches the license, the link key stays purely in VCDS.exe.
2. **Which VCDS methods are detoured?** Any in the crypto/key/link path?
3. **Recover `K_epoch`** by scanning the dumps for AES key schedules
   (`research/clb-crack/aes_ks_scan.py`); validate by decoding dump buffers / keystreams to
   the ground-truth above.
4. **Reverse the app-side KDF** from `(K, static_table, counter, b6-nonce)` located in the
   dumps → our own tool computes K → extensible driver.

## Environment note (crypto surface)

The process links **Windows CNG/bcrypt** (`bcryptprimitives.dll`, handles on `\Device\CNG`,
`\Device\KsecDD`, `rsaenh.dll`) — AES/key material may pass through BCrypt* APIs (round
keys in a BCRYPT_KEY object) in addition to / instead of LibTomCrypt. Consider both when
locating K in the dumps.

## Status
Fable agent `aae3244d...` is investigating the dumps + hook (prioritized on vcds_hook.dll).
