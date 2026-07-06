# VCDS-RUS crack + loader/hook architecture

The cracked-build artifacts, the loader/hook mechanism, and the Auto-Scan ground truth used
as the validation oracle. **Merges** `vcds-rus-crack-findings.md`, `loader-internals-findings.md`,
and `loader-mechanism.md`.

Session 2026-07-05/06. The owner runs **VCDS-RUS 24.7.1.0** (cracked Russian build, data
20240617 DS356.3) in an ARM Win11 VM (x86-on-ARM WOW64, `xtajit.dll`), cable passed through.
A full Auto-Scan succeeded → gave us ground-truth vehicle data AND 5 process memory dumps +
the crack's dropped files. This is the dynamic-attack material for recovering the OLD-scheme
link key `K_epoch` (see `research/clone-crypto.md`).

---

## 1. Ground truth from the Auto-Scan (validation oracle)

- **VIN: `XW8AD4NE9JH008917`**, chassis **NE-SK37 (3Q0)** = Škoda, mileage 212140 km.
- Scanned addresses: 01 02 03 08 09 10 15 16 17 19 42 44 52 5F 75.
- Key ECUs (addr → part / component / VCID):
  - 01 Engine `J623-CJSA`: SW `8V0 906 264 H`, HW `06K 907 425 B`, `1.8l R4 TFSI`,
    VCID `418318C6D089C39E84-8015`. Fault 15187 (P0850). Label `06K-907-425-V1.clb`.
  - 02 Auto-gearbox `J743` DQ200: SW `0CW 300 041 G`, comp `DQ200G2` `1003`.
  - 03 ABS `J104`: `5Q0 614 517 AQ`. 08 Climatronic `5E0 907 044 AM`.
  - 09 BCM `J519`: `5Q0 937 084 CF`. 19 Gateway `J533`: `3Q0 907 530 B` (holds VIN).
  - 17 Kombi `5E0 920 740 D`, 5F MIB `5E0 035 871 C`, 15 Airbag `3Q0 959 655 BE`, …

Any dump buffer / decoded `b7` that yields these ASCII/UDS bytes CONFIRMS a recovered `K`.
These are the golden fixtures for `vagcan info` regardless of transport.

---

## 2. Artifacts (in `research/dumps/`, gitignored — proprietary/PII)

- `VCDS.exe.dmp`, `VCDS-2/-3/-4.exe.dmp` — minidumps at random moments DURING the scan (link
  keyed, live AES round keys in memory). ~430 MB each.
- `VCDS-after-scan.exe.dmp` — after the scan finished (keys may be cleared).
- `vcds_hook.dll` (292 KB, x86 PE) and `VCDSLoader64.exe` (1.6 MB, x86-64) — the crack's files,
  dropped by the loader into Temp.

*(Note: the 5 minidumps were later deleted externally under disk pressure; a fresh dump is
needed for any renewed dynamic work.)*

---

## 3. The crack architecture (VCDS-RUS)

- **`VCDSLoader64.exe` injects `vcds_hook.dll` into VCDS.exe** — VCDS starts as a child of the
  loader, then detaches (owner-observed). Hook loads at VA **`0x1a730000`** (~1.14–1.19 MB
  in-memory; the on-disk drop is 292 KB packed).
- **`vcds_hook.dll` = FTD2XX PROXY SHIM + Delphi detour hook** ("Hook.32.dll"):
  - Exports the **full FT_\* surface** (87–89 exports: FT_Open/Read/Write/Close/ClrDtr/
    SetBitMode/ListDevices/…). It **replaces RT-USB/FTD2XX** — sits on the wire between VCDS
    and the real cable driver, seeing every FT_Read/FT_Write byte.
  - Non-FT exports `TMethodImplementationIntercept` + `dbkFCallWrapperAddr` → built with
    **Delphi + Cheat-Engine DBK / DDetours** (string `DDetours.TThreadsIDList` confirms inline
    in-process detours). So it also **detour-hooks VCDS methods in-process** — the
    license/genuineness patch that lets a clone/genuine cable run under the cracked build.
  - On-disk copy is **VMProtect-packed** (section `1111` entropy 7.92, `0000` bss `0xdb000`;
    same packer as VCDSLoader). Readable on-disk: export + import table only (advapi32
    RegCloseKey; KERNEL32 LoadLibraryA/GetProcAddress/VirtualProtect; netapi32 NetWkstaGetInfo;
    oleaut32 VariantCopy; user32 IsWindow; version VerQueryValueW). **Analyse the UNPACKED copy
    carved from a mid-scan dump at `0x1a730000`** (code-section entropy 6.61, all 291 pages
    present, MZ/PE intact), not the packed on-disk drop.
  - **The shim contains NO cryptography** (see `research/clone-crypto.md §3.4` for the full
    characterisation): no AES S-box/T-tables, no SHA init/RCON, no bcrypt/CNG/CryptoAPI imports;
    the `FT_*` handlers are `ret`-stubs / a detour pointer table. **FT_Read/FT_Write do not
    compute or inject `K_epoch`; the hook does NOT shortcut the KDF — the link key stays
    entirely inside VCDS.exe.**

### Environment note (crypto surface)
The process links **Windows CNG/bcrypt** (`bcryptprimitives.dll`, handles on `\Device\CNG`,
`\Device\KsecDD`, `rsaenh.dll`) — AES/key material may pass through BCrypt* APIs (round keys in
a `BCRYPT_KEY` object) in addition to / instead of LibTomCrypt. This is the basis of the
Tier-A CNG breakpoint plan in `research/clone-crypto.md §4.2`.

### Open questions the dumps were meant to answer (all now resolved — see `clone-crypto.md §3`)
1. Does the shim intercept/modify FT_Read/FT_Write to inject the key? → **No — pure proxy +
   license detour, no crypto.**
2. Which VCDS methods are detoured? → license/genuineness, not the crypto/key path.
3. Recover `K_epoch` by scanning the dumps for AES key schedules → **0 schedules in all 5
   dumps** (custom AES never leaves 240 contiguous cleartext round-key bytes).
4. Reverse the app-side KDF from `(K, static_table, counter, b6-nonce)` → blocked because `K`
   is unrecoverable from a static dump; needs live instrumentation (Probe 2).

---

## 4. `VCDSLoader.exe` — static classification (from `loader-internals-findings.md`)

**Goal of the pass:** determine what the loader targets (benign device/serial shim vs. patcher
of VCDS's interface-authentication). **Sample:** `research/VCDSLoader.exe`, 2,681,344 bytes,
timestamped 27 Feb 2022. *(This is the standalone loader; the crack ships the newer
`VCDSLoader64.exe` — §2.)*

### 4.1 File shape
| property | value | reading |
|---|---|---|
| Format | PE32, Intel 80386 (x86), GUI subsystem | 32-bit Windows app |
| Entry point | `0x0084c290` | non-standard section, not a normal `.text` |
| Section 0 | name `0000`, vaddr `0x401000`, size `0x218000`, CODE | junk name = packer artifact |
| Section 1 | name `1111`, vaddr `0x619000`, size `0x233600`, CODE+DATA | junk name = packer artifact |
| Section 2 | `.rsrc`, vaddr `0x84d000`, size `0x5b000` | resources + hijacked import table |
| Whole-file entropy | **7.546** | packed/encrypted |
| Section-0 code entropy | **7.835** | packed/encrypted (plain x86 sits ~6.0–6.5) |

**The loader is itself packed/protected** — junk section names (`0000`/`1111`), EP outside a
conventional code section, ~7.8 entropy, import table relocated into `.rsrc`. No plaintext
packer signature survived (stripped), but the structure is the same *class* of obstacle as the
VMProtect'd VCDS.

### 4.2 Imports / strings (what leaked through the packer)
Only the packer's **stub import set** is exposed (real imports resolve at runtime after the
unpack stub): `advapi32 comctl32 gdi32 KERNEL32 netapi32 ole32 oleaut32 shell32 user32 version
winspool.drv`. `advapi32` + `netapi32` (registry/HWID/network) are consistent with a
license/anti-tamper wrapper but not conclusive. Leaked strings: **`VirtualProtect`**
(memory-permission change — prerequisite for writing into code pages, i.e. runtime patching),
**`HHOOK`** (hints `SetWindowsHookEx`-style hooking), `VCDSLo…` (own name fragment). No FTDI /
`ftd2xx` / `d2xx` / VCP / `COM#` / `VID_`/`PID_` / latency strings; no version-info block.

### 4.3 What this establishes
**Confirmed: a self-protected x86 runtime patcher.** `VirtualProtect` + `HHOOK` + the protector
wrapping match a tool that edits another process's code — not a device/driver config utility
(which would not be packed like a protection tool and would show device/serial API strings;
neither holds). **Not determinable without unpacking:** the specific routine it patches in
VCDS (interface-authentication / clone-detection vs. other) — the real hook logic lives behind
the protector's unpack stub, exactly as VCDS's real code lived behind VMProtect.

**Decision:** do NOT implement or reconstruct the loader. Classifying benign-vs-auth would
require deriving the very thing (what it patches, where) that constitutes the working
circumvention. `vagcan` reaches the car through `vag-hex` (direct cable protocol) — no host
patch, no auth routine, no protector to defeat.

---

## 5. How a VCDS-style cable loader works, and why it breaks across versions (from `loader-mechanism.md`)

### 5.1 What a loader is
A **separate process** that starts the real application under its control and modifies the
app's in-memory code before (and sometimes during) execution. Two families: **launcher/patcher**
(starts target, edits RAM, hands control back — the on-disk `.exe` is untouched; the common
cable-loader shape: a small `*Loader.exe` beside a large host `.exe`) and **on-disk patcher**
(rewrites the `.exe` file permanently — rare, host updates/integrity checks clobber it). Cable
loaders are almost always **runtime memory patchers**.

### 5.2 Normal launch (no loader)
`OS loader → map PE → resolve imports (IAT) → TLS callbacks → entry point → app runs`. During
init or when a device is opened, the host runs an **interface-authentication routine** (queries
the USB device, checks vendor/product identity and/or a challenge–response, decides whether to
proceed). That routine is what a cable loader targets.

### 5.3 How the loader inserts itself
- **3a. Create suspended** — `CreateProcess(..., CREATE_SUSPENDED, ...)`: target mapped, no
  instruction executed — the clean window to edit code.
- **3b. Locate the code to patch** — the crux of version fragility. Either **fixed offset (RVA)**
  ("routine at image-base + N" — zero tolerance for change) or **signature / pattern scan
  (AOB)** (search mapped code for a supposedly-unique byte pattern, compute the patch site
  relative to the match — more resilient but still tied to the compiled shape). Real loaders
  usually use a signature scan with a fixed-offset fallback, because ASLR moves the image base
  every launch (absolute addresses recovered relative to the runtime base).
- **3c. Apply the patch** — **inline patch** (overwrite the check to always take "pass"),
  **inline hook / detour** (jump to injected code, run substitute logic, jump back), or **IAT
  hook** (swap a pointer so a call the app makes lands in loader code). Memory made writable
  (`VirtualProtectEx`), edited (`WriteProcessMemory`), permissions restored.
- **3d. Resume** — `ResumeThread`; the auth routine now returns "genuine" for the clone.

### 5.4 Why it worked on the old build
Everything in §5.3b–c is pinned to one exact compiled binary: the signature matched a real
unique location, the offset match→site was correct, the patched bytes matched the actual
instructions, and the packer/anti-tamper situation was what the loader tolerated. All four held.

### 5.5 Why the new build throws "hook error"
A "hook error" = the loader reporting "**I could not install my patch**" — almost always §5.3b
failed. Any one of these suffices:
- **5a. Recompilation moved/reshaped the code** — different register allocation, instruction
  selection, inlining, ordering, layout; the signature bytes no longer exist in that form →
  no match → abort. **Most common cause.**
- **5b. The check was restructured** — moved/split/re-stepped/changed success representation;
  a lucky partial match points at code that no longer does what the loader assumes.
- **5c. Different packer / added integrity protection** — packed builds reveal real code only
  after an unpack stub runs (timing problem); self-CRC/integrity checks detect the edit;
  anti-debug/anti-tamper trips on the process manipulation. *(The two VCDS binaries differ
  exactly here — one packed x86 under VMProtect, one unpacked ARM64 — which alone guarantees a
  loader for one can't map onto the other.)*
- **5d. Architecture / ABI change** — x86 → ARM64 (or 32→64): entirely different instruction
  set; x86 patch bytes are meaningless as ARM64 and vice-versa; the signature can't match
  because the encodings don't share an alphabet.
- **5e. ASLR + fixed-offset fallback** — if the signature scan fails and the loader falls back
  to an absolute offset, ASLR points it at unrelated memory (best case: clean "hook error";
  worst: patches garbage → later crash).

### 5.6 Mapping to the observed evidence
1. **Two structurally different VCDS binaries** — old packed x86 (VMProtect), new unpacked ARM64
   → §5c/§5d: a loader for the packed x86 build has no valid target in the ARM64 build.
2. **The address probe** — RE addresses derived from the *unpacked ARM64* image didn't line up
   against the *old packed x86* running process → §5a/§5c firsthand.
3. **"Hook error, then closes"** — textbook §5.3b failure: signature scan finds nothing → can't
   install → reports and exits rather than run unprotected.

**One-liner:** the loader hardcodes "recognize *this* code shape, patch *there*." A new version
changes the shape (recompile / restructure / repack / re-architect), recognition fails, and the
loader stops at "hook error." Nothing car-specific, nothing recoverable without re-deriving the
new build's internals — which is the bypass itself and out of scope.

### 5.7 Why this doesn't matter for `vagcan`
`vagcan` never runs VCDS — no host binary to patch, no auth routine to satisfy. The `vag-hex`
transport speaks the cable's own USB/serial protocol directly: no loader, no "hook error", no
version coupling. Same RE skill, aimed at the cable protocol instead of someone else's protected
binary.

---

## Cross-references
- `research/clone-crypto.md` — the encrypted-link crack, the dump memory-scan results, the full
  `vcds_hook.dll` characterisation, and the live probes.
- `research/vag-hex-framing.md` — wire format + link cipher.
- memory `[[vcds-cable-detect-re]]` — cable-detect RE + the cipher/auth facts.
