# Clone encrypted-link crack — the `K_epoch` saga

Canonical writeup of the clone HEX cable's encrypted diagnostic link and every route
attempted to recover its per-epoch AES session key. **Merges** `auth-mechanism-notes.md`,
`RE-PLAN-old-scheme-rekey.md`, `DYNAMIC-attack-playbook.md`, `DYNAMIC-attack-RESULTS.md`,
and `PATH2-vmprotect-dynamic-x86.md`.

Wire format, opcode vocabulary, the link cipher (`plain = cipher ^ KS`), `IV_TABLE`,
the `cid = (msg_type+1)&0xf` selector, off14/off15 rules, and the ISO-TP layer are all in
**`archive/research/vag-hex-framing.md`** — not repeated here. This doc is only the *auth /
session-key* problem.

> **One-line status.** The clone speaks the **OLD (b6/b7-derived) scheme**, whose
> per-ECU AES epoch key `K_epoch` is computed **app-side inside VMProtect-packed
> `VCDS.exe`**. Every *offline* route (static RE, replay-shortcut, memory-dump, the crack
> DLL) is exhausted; two *live* probes remain staged (full ordered replay; VMProtect
> dynamic instrumentation on a real x86 host). The new unprotected build's RSA-OAEP
> key-transport is a **different** scheme and does **not** apply to this cable.

---

## 1. The auth / handshake mechanism (from `auth-mechanism-notes.md`)

### 1.1 What it is
A cryptographic **challenge–response** run during session setup that proves to the VCDS
app that the cable is a **genuine Ross-Tech interface** (and lets the app reject
non-genuine ones). It is an authentication / anti-counterfeit measure — distinct from the
data-channel obfuscation cipher, which is interop. **Second consequence that matters to
us:** the AES-256 session key that encrypts the `b8`/`b7` diagnostic channel is a
**product of this exchange.**

### 1.2 Observable wire shape (HIGH — from the captures)
Position in the open sequence (`init-only.pcapng`, plaintext-observable):

```
02 probe → 04 identify ("ROSSTECH"+ver) → 82 → 0d          (plaintext bring-up)
→ b0 b1 b2 b3(x2) b4(x6) b5(x2)   each fe-acked            (setup burst)
→ frame#36  OUT 0xb6  payload ~24–27 bytes                 (CHALLENGE issued)
→ frame#37  IN  0xfe  ack
→ frame#38-39 IN 0xb7 payload 16 bytes ×2                  (RESPONSE(s))
→ frame#40  IN  0xb9  status/ack (2 bytes)
→ frame#41+ OUT b8 / IN b7 (16-byte units), b9/ff status   (session proceeds, ENCRYPTED)
```

| opcode | dir | shape | note |
|--------|-----|-------|------|
| `0xb6` | OUT | ~24–27 random-looking bytes | the challenge; entropy ~4.55 bits/byte over 8 samples |
| `0xb7` (init) | IN | 16-byte block(s) | the response(s); entropy ~3.95 bits/byte |
| `0xb9` | IN | `b9 40` (2 B) | flow/status ack around the exchange |
| `0xff` | IN | `ff 20` (2 B) | status / NAK-like |
| `0x09` | OUT↔IN | `09 <8B>` / `09 <7B>` | keyed exchange that RECURS through the session (see §2.2) |

**Shared container.** The 16-byte enciphered unit carrying the `0xb7` auth response is the
**same envelope** the diagnostic data channel (`0xb8`/`0xb7`) uses. Auth and data differ by
**function, not form** — same 16-byte block, different purpose.

### 1.3 Static loci (arm64 analysable proxy `VCDS-arm64-unpacked.exe`, ImageBase `0x140000000`)
- **Session-key install `0x140072ec0`** — a descriptor-driven key *import*: takes a
  caller-supplied blob (`x1`, len `w2`), decodes it via `0x14007ce68`, `memcpy`s the 32
  bytes into the cipher-context key slot `ctx+0x5da4`, and runs the AES-256 schedule
  (`0x14007b140`, IV table `0x140171d30`). **The key bytes come from the caller's blob, not
  any static literal.**
  - **CORRECTION (2026-07-05):** the earlier "the caller is reached only via a
    runtime-installed method pointer — no static `bl`" claim was **WRONG** (an artifact of a
    capstone bug: `md.disasm` halts at the first undecodable word, so the prior sweep saw
    ~422 of ~350k `.text` instructions). A word-by-word sweep (`xref.py`, `disfn.py`) finds
    **two direct `bl 0x140072ec0` callers**, both inside function **`0x14006d6c8`** (the
    received-block dispatcher): `0x14006d9d8` and `0x14006dc04`. Each installs the key from a
    stack buffer (`sp+0x30..`) holding a **plaintext `0xf0`-marked block** (`(byte&0xf0)==0xf0`
    guard at `0x14006d844`), with `x1 = sp+0x32`, `w2 = [sp+0x34]-3`; the decode reads
    `blob+3`. So the derivation trail is **statically reachable**, not hidden.
- **The decode `0x14007ce68` is a structured/length-driven decode:** a 128-byte-stride
  algorithm-descriptor table at `0x14055555c`, size-driven allocations (`0x1401318a0`),
  bit-length arithmetic (`w4 lsr 3`, `tst w4,#7`), and a multi-slot walk
  (`x21+0x50/0x68/0x98/0xb0`) in `0x14007d010`/`0x14007c858`. The 32-byte AES key is the
  *product* of this decode, not raw bytes lifted off the wire.
- **The old-scheme crypto is SYMMETRIC — no public-key primitive:** a name-string scan finds
  only THREE registered crypto primitives: **`aes`** (`0x14017ad80`), **`sha256`**
  (`0x14017ad94`), **`sprng`** (secure PRNG, `0x14017ae18`). No `ecc`/`rsa`/`dh`/`ecdh`/
  `x25519`/`curve25519`. Consequence: the derivation is very likely
  `key = KDF_sha256/aes( b6-challenge, b7-response, STATIC_APP_SECRET )` with a **static
  secret embedded in the binary** (the `0xb6` random bytes look like `sprng` output = the
  app's nonce). Potentially replicable by a live interop tool once the static secret + exact
  KDF are recovered — no PK wall. *(NB: this symmetric reading is for the OLD scheme the
  clone uses. The NEW unprotected build instead uses RSA-OAEP key-transport — see
  `vag-hex-framing.md` — and does NOT apply to this cable.)*
- **Genuineness signature compare:** fn `~0x140073380` parses an interface-response packet
  and compares a 6-byte cable signature against a hardcoded literal at `0x140073568` (bytes
  `01 00 00 c0 1e 00 00 00`); the reject path emits "This interface appears to have an issue."
  (string VMA `0x14017adc0`, xref `0x1400734f4`).
- **Presence/driver gate (separate, not auth):** `USB_Check` `0x1400747a8` — the SETUPAPI
  enumeration + D2XX driver-load path behind the "Driver/Interface Not Found" dialog.

### 1.4 Why the session key can't be synthesized offline (classification, not a method)
Passive black-box observation across the **two independent-session captures**: the same
logical diagnostic channel decodes to **identical UDS plaintext** but under **different
keystreams** (e.g. an RDBI channel: `reading-ecus` KS ≠ `init-only` KS, ciphertext
wholesale-different, frame-count fingerprint identical). With a static IV table and a fixed
channel selector, `keystream = AES(IV, key)` differing per session means the **AES key
changes every session**. The only per-session secret established at setup is the `0xb6`
challenge/response ⇒ the key is a product of the auth.

Two facts pinned (2026-07-05) that narrow the crack:
- **The session key is NOT present as clear bytes in the setup capture.**
  `crack_session_key.py` slides a 32-byte window over every setup-phase byte of
  `reading-ecus.pcapng` (all/OUT/IN concatenations) plus named blobs (`b6`, both `b7`, the
  `09` exchange) plus SHA-256 KDFs over every 1/2/3-way combination — **681 candidates**,
  each verified against the recovered `KS_F3` ground truth
  (`AES256(K).enc(IV_TABLE[4])[6:14] == 02 a9 99 f6 da 7c 9c 3a`). **Zero hits.**
- **You cannot back the key out of the keystream.** `KS_channel = AES_encrypt_block(K,
  IV_row)` is a single AES block; recovering `K` from a known `(IV_row → KS)` pair is exactly
  breaking AES-256. Obtaining `K` for a *new* live session requires reproducing the
  derivation.

Deliberately NOT recovered (the anti-counterfeit internals): the challenge algorithm (how a
valid `0xb7` is computed from a `0xb6`), the session-key derivation (how the blob fed to
`0x140072ec0` is built), and any response predictor / meaning of the 6-byte signature.

---

## 2. The `K_epoch` blocker (from `RE-PLAN-old-scheme-rekey.md`)

### 2.1 The blocker, precise
- Each diagnostic ECU rides its own **key epoch**. The capture has **40 `b6` events**; each
  opens exactly one channel (b6#1→`0x39` auth, #2→`0x9e`, #3→`0x43`, … #15→`0xf3`).
- A channel's keystream is `KS_cid = AES256(K_epoch).enc(IV_TABLE[cid])`, `K_epoch` fixed per
  epoch. We only have `K_epoch` **empirically** (per-channel keystreams recovered from
  known-plaintext), never the derivation.
- **Live, replaying the capture's 2nd `b6` does NOT re-key the cable** — it stays in epoch-1
  (byte-identical `0x39`/`0x38` blocks; the epoch-2 `0x9e` poll gets zero response, no wedge).
  Determinism holds only for the FIRST (fresh-cable) `b6`.
- ∴ we cannot open any ECU beyond epoch-1 session control until we can make the cable re-key.
  That derivation is the target.

### 2.2 `0x09` keyed exchange — investigated and KILLED as the re-key
Initial lead: shape OUT `09 <idx> <7 bytes>` (9 total) / IN `09 <7 bytes>` (8 total); a
**triplet idx 05,02,03 once per ECU-open**, plus a lone idx 01 during bring-up (seq 2) and at
seq 99. IN byte[3] is constant within an epoch's triplet and changes per epoch (reading-ecus
`0x48`→`0x71`→`0x80`; init-only `0x5e`→`0x61`), suggesting a per-epoch key-derived tag.

**UPDATE 1 (offline agent A) — `0x09` is NOT the re-key.** It fires at only **4 of 40** b6
epochs (#1,#8,#16,#23, cadence ~7–8), bundled with the `0x0b` reads → a periodic **anti-clone
attestation**, not per-ECU. ~36/40 epochs re-key with no `0x09` near them. IN pos3 is a
cable-chosen per-burst tag independent of OUT; IN is keyed (non-linear) but embeds cable
state, so not a pure function of OUT; the tag does **not** correlate with `K_epoch` (the
earlier "tag in keystream" was chance). Not crackable offline.

**KEY FINDING — the cable maintains ADVANCING internal crypto state.** The `0x0b` EEPROM
reads prove it: 8 fixed commands (`0b0000`…`0b0700`), yet every repeat returns a **different
41-byte response, all bytes varying** — the cable freshly randomizes per read. This explains
the replay results: epoch-1 replayed byte-for-byte because a fresh power-on **resets** the
cable state; the 2nd-`b6` replay went inert because the cable's state had already advanced
past the point the capture's 2nd `b6` applied. **Consequence:** replay-to-epoch via
*shortcuts* (`--deep`, sweep + partial POST_ADVANCE) is dead — any divergence desyncs the
cable's advancing state.

### 2.3 What is fully known (no need to re-derive)
- Wire framing, opcodes, link cipher, `IV_TABLE` (16 rows), selector `cid=(msg_type+1)&0xf`,
  off14 counter, off15 trailer — `vag-hex-framing.md`; encode/decode + ISO-TP implemented in
  `crates/vag-hex/src/link.rs`.
- Auth-advance: `0x39` completion `b8` off14 = `observed & 0xF8` (live-proven).
- New build = RSA-OAEP key transport (embedded RSA-1024 priv key `@0x140171a30`) — **NOT** what
  this clone uses; do not chase it for this cable.
- `b3`/`b4` = CAN addressing, `ID = (16-bit b4) >> 5`; engine `0x7E8` (`b4 …fd00`), gateway
  `0x77A` (`b4 …ef40`, always installed). VIN (DID F190) → engine/gateway.
- `0x0b` = encrypted 40-byte cable EEPROM, **NOT** a VIN cache.
- Cable hygiene: `FT_SetVIDPID(0x0403,0xFA24)` + FTDI init (8N1, 9600→19200→115200, DTR/RTS
  low, 1 ms latency) + join-worker-on-drop (clean `FT_Close`). Stable across runs.

### 2.4 The static ceiling (UPDATE 2, offline agent B — static RE)
- **RT-USB.dll** = verbatim stock FTDI FTD2XX (PDB `…d2xxdll…FTD2XX.pdb`, internal name
  `FTD2XX.dll`, full FT_* export surface, only `VID_0403&PID_6001`). Zero cable-protocol logic
  — the whole protocol is app-side in VCDS.exe. Dead end (confirmed).
- **VCDS.exe / VCDSLoader.exe** = fully **VMProtect**-packed: every code section entropy 8.00,
  virtualized IAT (1 static import/DLL), 0 protocol/crypto strings, no unpacked region. The
  OLD-scheme `0x09`/`b6`/`K_epoch` code is **NOT statically recoverable**.
- **arm64 build** (analysable proxy): command layer rides inside `0x04` data frames via encoder
  `0x14006cef0` → chokepoint `0x14006b640` → replies at `[ctx+0x204]`. Selector `(byte+1)&0xf`
  `@0x140073150` → IV_TABLE `@0x140171d30`. Wire `0x09` is a **transport-layer** handshake
  (state machine `0x14006c6a4`), NOT a command and NOT the key. Genuineness = cmd `0x45`
  `@0x140073df8` over a **static 128-byte table `@0x140171730` + a session counter
  `@0x1405e8108`** → matches "advancing state".
- **OLD-scheme `K_epoch` = symmetric KDF over (advancing session counter + static embedded
  table + b6/b7 nonce)** — best-supported model; sealed in VMProtect.

---

## 3. Offline routes exhausted

### 3.1 Static RE — sealed (see §2.4). VMProtect entropy-8, no unpacked region.

### 3.2 Replay-shortcut — dead (see §2.2)
The cable keeps advancing internal state; any non-exact replay desyncs. Only an **exact,
complete 1:1 replay from a fresh power-on** could work if the state is
deterministic-from-power-on (epoch-1's byte-for-byte reproduction suggests it may be) — this
survives as **live probe 1** (§4.1).

### 3.3 Memory-dump scan — PROVEN DEAD for this build (from `DYNAMIC-attack-RESULTS.md`)
**The plan** (`DYNAMIC-attack-playbook.md`): owner runs the old x86 VCDS + cable in an ARM
Win11 VM; dump `VCDS.exe` memory mid-scan (Process Hacker, or Task Manager → Create dump; RW
Private committed regions suffice); scan for the live AES key schedule to recover `K_epoch`
without a debugger, and locate the KDF inputs (`static_table@0x140171730`, session counter,
`b6`-nonce in a TX buffer) to reverse the KDF. (USBPcap is an x86/x64 kernel driver — won't
load on ARM64 Windows — so the **memory dump alone** was the vehicle; the wire-bytes fallback
would have been Windows ETW USB tracing, `wpr`/`logman`.) Tooling: `aes_ks_scan.py` /
`aes_scan_fast.py` (vectorised AES-128/192/256 schedule scanner), `scan_dump_keys.py`
(minidump-region-aware, VA/module mapping), `validate_k.py`.

**The result — the memory-scan path does NOT work for this build.** Analysed all 5 live
minidumps (`research/dumps/{VCDS,VCDS-2,-3,-4,VCDS-after-scan}.exe.dmp`, Windows minidumps of
the x86 process under ARM WOW64, one Auto-Scan on the owner's Škoda VIN `XW8AD4NE9JH008917`,
429–436 MB each):

1. **No AES key schedule of ANY size (128/192/256), in standard OR LibTomCrypt word-swapped
   byte order, in ANY of the 5 dumps.** The scanner is verifiable (a 240/208/176-byte window
   either IS a valid FIPS-197 expansion or is not) and passes synthetic self-tests for all
   three sizes and both layouts. Result: **0 keys.** The fast scanner's entropy pre-filter
   (`len(set(key)) >= 12` for AES-256; 10/8 for 192/128) cannot reject a real key (a
   random/KDF-derived 32-byte key has ≈30 distinct bytes), so the 0-result is conclusive.
2. **No XOR-valid `b8`/`b7` link frames in any dump** (strict `53 14 b8 <16> xor` /
   `4d 14 b7 <16> xor` → 0 hits). Raw wire frames are consumed and discarded.
3. The dumps hold only **high-level decoded application data** — VIN + every part-number as
   length-prefixed Delphi strings (VIN as `11 00 00 00` + `XW8AD4NE9JH008917`, ASCII +
   UTF-16LE, all 5 dumps). The link-layer 16-byte plaintext blocks and raw UDS PDUs
   (`62 f1 90 …`) are NOT adjacent — the wire data is fully lifted into app strings before the
   dump moment.

∴ the playbook's central assumption ("the expanded AES schedule sits in cleartext in the
cipher context") is **false for this build.** The link cipher is a custom/table AES statically
linked *inside* the VMProtect-packed VCDS.exe; its round keys are never materialised as 240
contiguous cleartext bytes on a normal heap (recomputed per-frame in VM context, or held in a
VMProtect-managed region). `K_epoch` remains sealed.

**Cross-dump summary** — AES keys / XOR-valid b8·b7 / decoded VIN+part# present:

| dump | when | AES keys | XOR-valid b8/b7 | VIN/part# present |
|------|------|---------:|----------------:|-------------------|
| VCDS            | mid-scan  | 0 | 0 | yes |
| VCDS-2          | mid-scan  | 0 | 0 | yes |
| VCDS-3          | mid-scan  | 0 | 0 | yes |
| VCDS-4          | mid-scan  | 0 | 0 | yes |
| VCDS-after-scan | post-scan | 0 | 0 | yes |

**Positive locations recovered (for the KDF, if VCDS.exe is ever unpacked).** The OLD x86
VCDS.exe embeds the SAME static tables as the analysable ARM64 build:

| item | ARM64 VA | found in VCDS-2 dump at | live VA / module |
|------|----------|-------------------------|------------------|
| 16-row link-cipher `IV_TABLE` | `0x140171d30` | file off `0x271646` (16 contiguous rows, byte-identical) | `0x5384d0` [VCDS.exe] |
| 128-byte genuineness/static table | `0x140171730` | file off `0x2ad62e` (byte-identical) | `0x5744b8` [VCDS.exe] |

So the KDF's static-table input is confirmed present and identical to the ARM64 table. What
is still missing to build the KDF tuple `(K, static_table, counter, b6-nonce)` is `K` itself
(not in memory) and the counter/nonce loci (not pursued once `K` proved unrecoverable).

### 3.4 The crack DLL (`vcds_hook.dll`) — characterised, NOT a shortcut
The crack (VCDS-RUS) injects `vcds_hook.dll` (VA `0x1a730000`, ~1.19 MB, present in every
dump). Carved from the mid-scan dump (all 291 pages present; MZ/PE intact). **In-memory it is
unpacked** (code-section entropy 6.61; the on-disk copy is VMProtect-packed, section "1111"
entropy 7.92, code-section rawsize 0).

- **It is "Hook.32.dll" — a transparent FTDI/FTD2XX proxy shim.** Exports the entire 87-function
  `FT_*` surface (FT_Open/Read/Write/ListDevices/SetBitMode/…) plus two Delphi exports
  `TMethodImplementationIntercept` + `dbkFCallWrapperAddr`. Built with **Delphi + DDetours**
  (Cheat-Engine DBK) — string `DDetours.TThreadsIDList` confirms inline in-process detours. It
  sits between VCDS.exe and the real RT-USB.dll, forwarding `FT_*` and detour-patching VCDS
  methods (the license/genuineness bypass that lets a clone run under the cracked build).
- **The shim contains NO cryptography.** No AES S-box/T-tables, no SHA-1/256 init or round
  constants, no `RCON` (searched the full unpacked image). No `bcrypt`/CNG/CryptoAPI imports
  (imports are only advapi32/kernel32/netapi32/oleaut32/user32/version; delay-imports are 5
  unrelated OS functions). The exported `FT_*` handlers are `ret`-stubs / a pointer table
  (detour trampolines), not crypto.

**Conclusion:** FT_Read/FT_Write do not compute or inject `K_epoch`; the shim detours
in-process VCDS methods via DDetours, but those are the license/genuineness patch, not the
link-key path; it calls no bcrypt/AES. **The link key stays entirely inside VCDS.exe — the
hook does NOT shortcut the KDF.**

---

## 4. The two live probes (staged)

### 4.0 The success oracle (define first — every route ends here)
Recovered key material is believed **only** once it decodes to the ground-truth vehicle (from
`vcds-rus-crack.md`, the owner's Škoda):
- **VIN `XW8AD4NE9JH008917`**, chassis NE-SK37 (3Q0).
- Engine 01 `J623-CJSA` SW `8V0 906 264 H`, HW `06K 907 425 B`, `1.8l R4 TFSI`.
- Gateway 19 `J533` `3Q0 907 530 B` (holds VIN). BCM 09 `5Q0 937 084 CF`. ABS 03
  `5Q0 614 517 AQ`.

Link-cipher relation a correct `K` must satisfy:
```
KS_cid  = AES256_ECB(K_epoch).encrypt( IV_TABLE[cid] )     # 16-byte keystream per channel
cid     = (msg_type + 1) & 0xF                             # 16 channels
plain[i]= cipher[i] XOR KS_cid[i]                          # byte-local XOR, i = 0..15
```
Engine diagnostic channel = **`KS_F3`** = block `0xf3` → `cid = (0xf3+1)&0xf = 4` →
`IV_TABLE[4]`. `link_cipher.py` already reproduces `KS_F3[6..13] = 02 A9 99 F6 DA 7C 9C 3A`
from known plaintext, so a recovered `K` for the `f3` epoch must satisfy
`AES256_ECB(K).encrypt(IV_TABLE[4])[6..13] == 02 a9 99 f6 da 7c 9c 3a`.

Oracle command (canonical — do not reinvent; `validate_k.py` exists):
```bash
cd research/clb-crack
# 1) fastest sanity — does K reproduce the known f3 keystream?
.venv/bin/python - "$KHEX" <<'PY'
import sys; from link_cipher import IV_TABLE
from Crypto.Cipher import AES
K = bytes.fromhex(sys.argv[1].replace(" ",""))
ks = AES.new(K, AES.MODE_ECB).encrypt(IV_TABLE[4])
print("KS_F3[6..13] =", ks[6:14].hex(" "),
      "  EXPECTED 02 a9 99 f6 da 7c 9c 3a",
      "  MATCH!" if ks[6:14].hex()=="02a999f6da7c9c3a" else "  no match")
PY
# 2) full oracle — decode framed b8/b7 blocks in a live dump to UDS structure
.venv/bin/python validate_k.py <a-fresh-dump-of-the-same-epoch>.dmp "$KHEX" --show
```
Caveat: `K_epoch` rotates per `b6` epoch; the stored `KS_F3` crib is the **engine epoch** from
`reading-ecus.pcapng`. To validate a *live* `K`, either trigger the same engine `f3` epoch and
compare, or dump a fresh `b8`/`b7` frame in the *same* session you pulled `K` from and run
`validate_k.py` — key and ciphertext then belong to one epoch and must agree.

### 4.1 Probe 1 — exact-complete 1:1 replay from a fresh power-on (`vagcan replay-drive`) [CABLE]
No new hardware. Replays the **entire** recorded OUT-frame sequence verbatim (every
state-advancing frame incl. the `0b` reads, in order, counters restamped) from a **cold cable
power-on** — unlike all prior partial-sequence shortcuts that desynced early (§2.2/§3.2). If
the cable's crypto state is deterministic-from-power-on, the replay tracks it to the engine
`f3` channel (idx 1045, epoch #15) where the recovered `KS_F3` applies → inject a crafted UDS
`22 F1 90` and decode the VIN. If the state is CSPRNG-fresh, it emits the **exact divergence
index** (expected vs observed) — a clean empirical verdict. This is the **highest-probability
path without unpacking**. Run: `cargo run -p vagcan -- replay-drive --stream
research/dumps/replay-stream.jsonl` (fresh power-on first).

### 4.2 Probe 2 — VMProtect dynamic on a real x86 host (from `PATH2-vmprotect-dynamic-x86.md`)
Pull `K_epoch` out of the **running** cracked VCDS-RUS (old x86 VMProtect build — the one the
clone actually speaks) on a **native x86-64 Windows** machine, where hardware DR breakpoints,
Intel Pin and Triton actually work. This **supersedes the offline memory-scan** (§3.3, proven
dead): a live attack follows the *pointer the code uses*, valid wherever the key lives
(VMProtect-guarded region, CNG object, stack, T-table context) — the frozen-snapshot negative
says little about it.

#### Environment
- **#1 REQUIREMENT — a REAL x86 host, not the owner's ARM VM.** VMProtect 3.6+ detects **both**
  hardware-hypervisor **and** CPU emulation (CPUID hypervisor bits + vendor leaf, Trap-Flag /
  RDTSC timing, SMBIOS / BIOS Option-ROM firmware tables). The ARM-Parallels + x86-emulation VM
  is doubly detectable and has no real DRx / no Pin. Use **bare-metal native x86** (32-bit-
  friendly Win10/11); fallback = a hardened **VirtualBox ≥ 7.0.4** (older builds trip an
  EIP-checking bug); avoid **Heaven's Gate** WOW64 x86-on-x64 transitions (run a native 32-bit
  context for the 32-bit VCDS.exe).
- **Same binary.** Copy the owner's OLD x86 `VCDS-RUS 24.7.1.0` (data 20240617 DS356.3)
  verbatim: `VCDS.exe`, `VCDSLoader.exe`/`VCDSLoader64.exe`, `RT-USB.dll`, and the crack's
  `vcds_hook.dll`. Cable straight into host USB (`VID_0403`; app sets PID `0xFA24`, driver
  expects `PID_6001`). The hook carries no crypto (confirmed §3.4) — neither helps nor hinders.
- **Trigger a fresh re-key** for the trace: connect → open **engine (01)** → do an RDBI
  (SW-version / measuring-blocks screen). That drives the `b0..b5` burst → a fresh `b6` → the
  app derives `K_epoch` → per-frame `KS_cid` starts. Arm breakpoints/Pin trace *before* the open.
- **Toolchain:** x64dbg (x32dbg for this 32-bit target) + **ScyllaHide**; WinDbg alt; **Intel
  Pin** (kit ≥ 3.28, MSVC); **Triton**; **Detect It Easy**; **Keystone**.
- **Rebase.** `VCDS.exe` is a 32-bit Delphi PE, preferred `ImageBase 0x400000`. `IV_TABLE` live
  VA `0x5384d0` → **RVA `0x1384d0`**; 128-byte table live VA `0x5744b8` → RVA `0x1744b8`. With
  ASLR use `module_base + RVA`. **Verify** by dumping 16 bytes at `iv` == `IV_TABLE[0]` =
  `56 51 54 3b 24 45 15 21 03 54 1a 34 54 82 10 4c`; row 4 (engine `f3`) at `iv + 0x40` =
  `4d 39 86 a3 de e2 ba 2a d0 4c 1c df 23 34 45 ee`.

#### Static anchors (put in the debugger up front)
| item | arm64 VA (proxy) | old x86 live VA (base 0x400000) | RVA |
|------|------------------|----------------------------------|-----|
| 16-row link-cipher `IV_TABLE` | `0x140171d30` | **`0x5384d0`** (byte-identical, dump VCDS-2 file off `0x271646`) | `0x1384d0` |
| 128-byte genuineness/static table | `0x140171730` | **`0x5744b8`** | `0x1744b8` |
| session counter | `0x1405e8108` | *(find x86 equivalent — Tier B.6)* | — |
| channel selector `(byte+1)&0xf` | `0x140073150` | *(virtualized in x86)* | — |
| AES-encrypt block (arm64 native T-table) | `0x1400780a8` | *(x86 analogue = Tier-A target)* | — |

Engine channel: `cid=4`, keystream `KS_F3`, `IV_TABLE[4]` @ live VA **`0x538510`**.

#### The five reference links — honest verdicts
Recurring theme: native x86 tools **UNPACK** (clean image + OEP + IAT) but do **not**
devirtualize. Only the hackyboiz Pin+Triton method truly devirtualizes; link 5 is the
anti-debug enabler for any live attach on VMProtect 3.6+.

| link | what it is | verdict for our target |
|------|-----------|------------------------|
| 1. `void-stack/VMUnprotect.Dumper` | **.NET-only** VMProtect dumper | **NOT applicable** — VCDS.exe is native x86-32 Delphi. |
| 2. `sudha2323/vmprotectunpacker` | native runtime dump-unpacker (INT3 at OEP → dump → Capstone) | Defeats packing, not virtualization → feeds **Tier 0**. |
| 3. `hackyboiz` VMPpart2 (Pin + Triton) | **true devirtualization** (DIE → Pin trace → cluster handlers → Triton lifts → reconstruct/Keystone) | **The only true devirt — backbone of Tier B.** |
| 4. `muhammadh772` Delphi-VMProtect unpacking | native Delphi VMP unpack (bp VirtualAlloc/Protect → OEP → dump → Scylla IAT) | **Clean image only** → the recipe for **Tier 0**. |
| 5. `cyber.wtf` "Defeating VMProtect's latest tricks" | defeating VMProtect 3.6+ anti-analysis (kernel-level) | **The anti-debug checklist** that makes attaching possible → Tier A prerequisites. |

#### Tier 0 — produce a clean UNPACKED x86 image (enabler for A and B)
We have an unpacked **arm64** proxy but **no** unpacked x86 image. Tier 0 gets one (static-
analysis parity on the real target — to place the KDF VM entry, the x86 session counter, the
set-key analogue). UNPACKS only; does **not** devirtualize. Recipe (link 4): launch under
x32dbg+ScyllaHide → bp `VirtualAlloc`/`VirtualProtect` (or `Nt*`) to catch the unpack → find
OEP (Delphi prologue `push ebp; mov ebp,esp`) → dump → **Scylla** rebuild IAT → load in
IDA/Ghidra. Deliverable: `VCDS-x86-unpacked.exe`.

#### Tier A — HW-breakpoint the native AES keystream generator (cheap, try FIRST)
**Hypothesis:** vendors virtualize the **KDF** but rarely the **bulk per-frame AES** (~100×
VM slowdown). So `KS_cid = AES(K).enc(IV_TABLE[cid])` is plausibly native; when it runs it
reads a row of `IV_TABLE` as the plaintext block. Three sub-targets, best first:

1. **CNG / BCrypt (strongest, native by definition).** The process links
   `bcryptprimitives.dll` and touches `\Device\CNG`. If AES routes through CNG, the **key is
   imported in the clear**:
   - `bcryptprimitives!BCryptGenerateSymmetricKey` — arg `pbSecret`/`cbSecret` = the **raw
     32-byte `K`** (the prize).
   - `bcrypt!BCryptEncrypt` — `hKey` handle; or capture `pbInput` (should equal an `IV_TABLE`
     row) + `pbOutput` (the keystream) to confirm the relation even without `K`.
   CNG is an OS DLL — never virtualized. Also explains the FIPS-197 scan negative (CNG holds
   the expanded key in an opaque `BCRYPT_KEY` object, not a 240-byte heap array).
2. **`IV_TABLE`-read HW breakpoint (works for any AES backend).** Memory **read** DR breakpoint
   on the engine row `IV_TABLE[4]` (live VA `0x538510`, or `module_base + 0x1384d0 + 0x40`). It
   fires at the instruction consuming the IV as the AES plaintext. **Native AES** (S-box /
   T-table, `xor`/`rol`, 240-byte round-key window via a register/stack pointer) → dump round
   keys / set a second read-bp on the round-key buffer and grab the 32-byte master `K`. **VM
   dispatch** (fetch → jump-table → handler interpreter) → AES is *also* virtualized → **fall
   through to Tier B** (this outcome *diagnoses* virtualization, the precondition Tier B needs).
3. **Native key-schedule buffer** (if 1 gives nothing and 2 shows native AES): the round-key
   pointer is a register/stack arg; first 32 of the 240 bytes = master `K`.

Tier A prerequisites (link 5, getting a debugger attached to VMProtect 3.6+): cover **direct
syscalls** (sysenter/`syscall` bypass user-mode API hooks — ScyllaHide build that handles them
+ pin to a known OS build); **hypervisor + emulation detection** (native bare-metal avoids it);
**`KUSER_SHARED_DATA`** timing checks; **API hooks** `NtQueryVirtualMemory`, `NtOpenFile`,
`NtCreateSection`, `NtMapViewOfSection` (plus classic `NtQueryInformationProcess`
ProcessDebugPort/Flags/Object, `NtSetInformationThread` HideFromDebugger, `IsDebuggerPresent`);
avoid Heaven's Gate. Debugger-side: ScyllaHide VMProtect profile on **before** launch; HW DR
breakpoints (stealthier than INT3 — no code-byte edits, so self-CRC doesn't trip; use
ScyllaHide "protect DRx" against DR7 reads); prefer launch-suspended-then-arm, or attach to the
already-running child if `VCDSLoader` injects `vcds_hook.dll` first.

x64dbg recipe: launch x32dbg + ScyllaHide → open VCDS.exe (or attach) → `scriptload
research\clb-crack\hwbp_ivtable.x64dbg.txt` (rebases, verifies `IV_TABLE`, arms the HW read-bp
on row 4, drops BCrypt breakpoints) → in VCDS open Engine 01 + trigger a read → at the hit
follow EIP, native AES vs VM dispatch, dump 32 (or 240→first 32) bytes → oracle §4.0. WinDbg
alt: `$$><research\clb-crack\hwbp_ivtable.windbg.txt`.

#### Tier B — full devirtualization of the KDF (heavy; only if Tier A shows virtualization)
Follows the **hackyboiz Pin + Triton** pipeline. B.0 DIE version-detect + confirm virtualization
mode. B.1 find the VM entry for the KDF (arm64 chain dispatcher `0x14006d6c8` → set-key
`0x140072ec0`; the x86 KDF sits in the analogous session-setup region and pushes into the VM —
mark the `push`/`call` into the interpreter). B.2 Pin trace via `kdf_trace.pin.cpp` (skeleton
provided) → `vmtrace.out` (every executed instruction), `bytecode_values.txt` (operand words
read from the VM bytecode pointer), `handler_registers.txt` (register snapshot per handler);
build MSVC x86 `make TARGET=ia32`, run `pin -t kdf_trace.dll -- VCDS.exe`, trigger one
engine-open so exactly one KDF runs. B.3 handler ID & clustering (`uniq -c` → dispatcher =
highest-frequency address; cluster identical byte sequences → one canonical rep per handler).
B.4 Triton lifts each handler's semantics via `kdf_triton.py` (loads a canonical segment + entry
register snapshot, executes symbolically, diffs pre/post state to name `LCONST`/`ADD`/`XOR`/
`ROL`/load/store/bytecode-advance); hackyboiz recovered constant-decrypt chains like:
```
val = (enc + 0x55106798) & 0xFFFFFFFF
val = (-val) & 0xFFFFFFFF
val = (val + 0x69733a52) & 0xFFFFFFFF
val = ((val << 1) | (val >> 31)) & 0xFFFFFFFF
dec = ~val & 0xFFFFFFFF
```
B.5 reconstruct the KDF & reimplement in Rust (we don't need to patch `vir_Entry` — only the
*algorithm* so vagcan computes `K_epoch` itself), tied to the model
`K_epoch = KDF( advancing_session_counter , static_table@0x5744b8 , b6/b7 nonce )` — a future
`crates/vag-hex/src/kdf.rs`, gated by the §4.0 oracle. B.6 locate the x86 session counter
(arm64 `0x1405e8108` analogue) via the Pin trace (the monotonic per-epoch value the KDF reads
that is neither table nor nonce) + a HW write-bp diff. B.7 deliverables: `kdf_trace.pin.cpp`,
`kdf_triton.py`, the recovered `opcode→semantics` table + KDF pseudo-code, and `kdf.rs`.

#### Risk / effort per tier
| tier | effort | success prob | key risk | payoff |
|------|--------|-------------|----------|--------|
| **0 — clean unpacked x86 image** | ~hours–day | high (link 4) | anti-debug attach cost (link 5) | static parity → places KDF VM-entry & counter; NOT `K` |
| **A1 — CNG BCrypt bp** | ~hours | high **if** AES uses CNG | VCDS may use in-house LibTomCrypt AES → no BCrypt hit | raw 32-byte `K` from `BCryptGenerateSymmetricKey` |
| **A2 — IV_TABLE read HW-bp** | ~hours | medium-high | AES may be virtualized → bp in VM loop (but this *diagnoses* it) | round-key pointer / `K`; or a definitive "virtualized" verdict |
| **A3 — native schedule dump** | ~hours | medium | needs A2 to show native AES | first 32 of the 240-byte schedule = `K` |
| **B — full devirt of KDF** | days–weeks | medium | handler-zoo size; anti-debug during long traces; isolating the KDF VM-entry | the *algorithm* → vagcan computes any epoch's `K` forever |

Order: A1 and A2 in the **same** debugger session, then A3 if A2 shows native AES. Escalate to
B only if A2/A1 prove virtualization.

**Honest confidence: ~55–65% that Tier A yields `K` directly.** *For:* per-frame bulk AES is
the classic thing left native; the process links CNG (key import in an OS DLL); a live HW-bp
follows the pointer the code uses (works even if the round keys live in a VMProtect-guarded
region). *Against:* x86 `.text` reads entropy 8.00 with no unpacked region at rest; the 5-dump
scan found zero schedules (weak live evidence — snapshot vs pointer-follow — but non-zero: a
native LibTomCrypt AES writing a 240-byte heap context would plausibly have been caught, and
was not, nudging toward CNG-opaque or virtualized). Even a "failure" is a *diagnosis*. **The
CNG breakpoint is the highest-expected-value single action — run it first.**

---

## 5. Recommendation / decision

Three routes to a live VIN (all offline-crack routes now closed):
1. **Probe 1 — exact-complete 1:1 replay from fresh power-on** [CABLE] (§4.1). Highest-
   probability path without unpacking; reuses everything built; one fresh-power-on session
   tests it. **Try first.**
2. **Probe 2 — VMProtect dynamic** (§4.2). Substantial separate effort; no cable to crack, only
   to use it. The "crack the clone properly" long game.
3. **Generic USB-CAN bypass** (`vag-can` + a cheap slcan dongle) — UDS-over-ISO-TP-over-CAN
   straight to the car, sidesteps the clone link entirely. **The pragmatic route to
   `vagcan info`**, and the original designed fallback.

If Probe 1 diverges (true late-session randomness), fall back to the generic-CAN product path;
keep Probe 2 as the long game.

---

## Tooling (all in `research/clb-crack/`, run with `.venv/bin/python`)
`usbpcap.py` (frame reassembly), `link_cipher.py` (IV_TABLE, keystream recovery),
`crack_session_key.py`, `validate_k.py` (K → keystream → UDS decode oracle),
`aes_ks_scan.py`/`aes_scan_fast.py`/`scan_dump_keys.py` (dump AES-schedule scanners),
`disfn.py`/`xref.py` (AArch64 sweeps), `extract_rsa_key.py`, `extract_replay_stream.py`,
`hwbp_ivtable.{x64dbg,windbg}.txt`, `kdf_trace.pin.cpp`, `kdf_triton.py`.
