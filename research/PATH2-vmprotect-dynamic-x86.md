# PATH 2 — recover `K_epoch` by DYNAMICALLY attacking VMProtect-packed VCDS.exe on a real x86 Windows host

Runnable playbook for pulling the OLD-scheme per-epoch AES session key `K_epoch`
out of the **running** cracked `VCDS-RUS` (old x86 VMProtect build — the one the
clone cable actually speaks) on a **native x86-64 Windows** machine, where
hardware DR breakpoints, Intel Pin and Triton actually work (unlike the owner's
ARM x86-emulation VM, which has no real DRx and no Pin).

This supersedes the offline memory-scan route, which is **proven dead** for this
build: `research/DYNAMIC-attack-RESULTS.md` scanned all 5 live minidumps (429–436 MB
each, standard + LibTomCrypt word-swapped layouts, AES-128/192/256) and found **zero**
key schedules and **zero** XOR-valid `b8`/`b7` frames. The expanded round keys are
never 240 contiguous cleartext bytes on a normal heap in a frozen snapshot. A live
attack that follows the *pointer the code actually uses* is a different experiment —
that pointer is valid wherever the key lives (VMProtect-guarded region, CNG object,
stack, or T-table context), and the frozen-snapshot negative says little about it.

---

## 0. The success oracle (define first — every tier ends here)

Recovered key material is **only** believed once it decodes to the ground-truth
vehicle. From `research/vcds-rus-crack-findings.md`, the owner's own Škoda:

- **VIN `XW8AD4NE9JH008917`**, chassis NE-SK37 (3Q0).
- Engine 01 `J623-CJSA` SW `8V0 906 264 H`, HW `06K 907 425 B`, `1.8l R4 TFSI`.
- Gateway 19 `J533` `3Q0 907 530 B` (holds VIN). BCM 09 `5Q0 937 084 CF`. ABS 03 `5Q0 614 517 AQ`.

### The link-cipher relation (what a correct `K` must satisfy)

```
KS_cid  = AES256_ECB(K_epoch).encrypt( IV_TABLE[cid] )      # 16-byte keystream per channel
cid     = (msg_type + 1) & 0xF                              # 16 channels
plain[i]= cipher[i] XOR KS_cid[i]                           # byte-local XOR, i = 0..15
```

- `IV_TABLE` = 16 rows × 16 bytes, embedded verbatim in `research/clb-crack/link_cipher.py`.
- Engine diagnostic channel = **`KS_F3`** = block `0xf3` → `cid = (0xf3+1)&0xf = 4` → `IV_TABLE[4]`.
- The engine `f3` channel is the one whose keystream `research/clb-crack/link_cipher.py`
  already reproduces from known plaintext (`KS_F3[6..13] = 02 A9 99 F6 DA 7C 9C 3A`),
  so it is the tightest cross-check: a recovered `K` for the `f3` epoch must satisfy
  `AES256_ECB(K).encrypt(IV_TABLE[4])[6..13] == 02 A9 99 F6 DA 7C 9C 3A`.

### The oracle command

```bash
cd research/clb-crack
# 1) Fastest sanity check — does K reproduce the known f3 keystream? (offset 6..13)
.venv/bin/python - "$KHEX" <<'PY'
import sys; from link_cipher import IV_TABLE
from Crypto.Cipher import AES
K = bytes.fromhex(sys.argv[1].replace(" ",""))
ks = AES.new(K, AES.MODE_ECB).encrypt(IV_TABLE[4])
print("KS_F3[6..13] =", ks[6:14].hex(" "),
      "  EXPECTED 02 a9 99 f6 da 7c 9c 3a",
      "  MATCH!" if ks[6:14].hex()=="02a999f6da7c9c3a" else "  no match")
PY

# 2) Full oracle — decode framed b8/b7 blocks in a live dump to UDS structure
.venv/bin/python validate_k.py <a-fresh-dump-of-the-same-epoch>.dmp "$KHEX" --show
```

A `KS_F3` match on step 1 (or UDS-looking PCI+SID / `62 f1 90` VIN decodes on step 2)
= **`K_epoch` CONFIRMED.** `validate_k.py` already exists and is the canonical test;
do not reinvent it.

> Caveat baked into the model: `K_epoch` rotates per `b6` epoch. The stored `KS_F3`
> crib is the **engine epoch** from `reading-ecus.pcapng`. To validate a *live*
> recovered `K`, either (a) trigger the same engine `f3` epoch and compare to the
> stored crib, or (b) dump a fresh `b8`/`b7` frame in the *same* session you pulled
> `K` from and run `validate_k.py` on that dump — the key and the ciphertext then
> belong to one epoch and must agree.

---

## 1. Environment setup on the real x86 host

0. **#1 REQUIREMENT — a REAL x86 host (not the owner's ARM VM).** VMProtect 3.6+
   detects **both** hardware-hypervisor **and** CPU emulation (link 5): CPUID hypervisor
   bits + vendor leaf, Trap-Flag / RDTSC timing anomalies, and SMBIOS / BIOS Option-ROM
   firmware-table fingerprints. The owner's ARM-Parallels + x86-emulation VM is **doubly
   detectable** (emulation *and* hypervisor) and also has no real DRx / no Pin — Tier A/B
   simply cannot run there. Use **bare-metal native x86** (32-bit-friendly Win10/11) if
   at all possible; if a VM is unavoidable, a *carefully hardened* **VirtualBox ≥ 7.0.4**
   (older builds have an EIP-checking bug VMP trips on) with anti-detection tuning, and
   avoid **Heaven's Gate** WOW64 x86-on-x64 transitions (link 5 deliberately used a native
   32-bit system to sidestep them). Native bare-metal is strongly preferred.
1. **Same binary.** Copy the owner's OLD x86 `VCDS-RUS 24.7.1.0` (data 20240617
   DS356.3) install verbatim to the native x86-64 Win10/11 box — same `VCDS.exe`,
   `VCDSLoader.exe`/`VCDSLoader64.exe`, `RT-USB.dll`, and the crack's `vcds_hook.dll`.
   This is the build the clone cable keys against (the new build uses RSA-OAEP and
   is NOT this clone — do not chase it).
2. **Cable passthrough.** Plug the clone HEX cable straight into the native host's
   USB (no VM layer). Confirm FTDI enumerates (`VID_0403`; the app sets PID `0xFA24`
   / driver expects `PID_6001`). The crack's `vcds_hook.dll` proxies FTD2XX so the
   clone passes genuineness — it carries **no crypto** (confirmed), so it neither
   helps nor hinders key recovery; `K` lives entirely in `VCDS.exe`.
3. **Trigger a fresh re-key / epoch for the trace.** Each ECU-open is its own `b6`
   epoch (b6#1→auth `0x39`, … b6#15→engine `0xf3`). To make VCDS derive a *new*
   `K_epoch` on demand: connect → open the **engine (01)** controller → do a
   ReadDataByIdentifier (e.g. the SW-version / measuring-blocks screen). That drives
   the `b0..b5` addressing burst → a fresh `b6` → the app derives `K_epoch` → per-frame
   `KS_cid = AES(K).enc(row)` starts flowing. Breakpoints (Tier A) or the Pin trace
   (Tier B) must be armed *before* this open so they catch the derivation/first encrypt.
4. **Toolchain.** Install: x64dbg (x32dbg for this 32-bit target) + **ScyllaHide**
   plugin; WinDbg (x64) as the alternate; **Intel Pin** (kit ≥ 3.28, MSVC toolchain);
   **Triton** (pip `triton-library` or the built DLL); **Detect It Easy (DIE)**;
   **Keystone** (`pip install keystone-engine`) for Tier B reconstruction.
5. **Confirm the module & rebase (do this once, both tiers need it).** `VCDS.exe`
   is a 32-bit Delphi PE, preferred `ImageBase 0x400000`. The stated live VAs assume
   no ASLR slide (base `0x400000`):
   - `IV_TABLE` live VA `0x5384d0` → **RVA `0x1384d0`** (`0x5384d0 - 0x400000`).
   - 128-byte genuineness/static table live VA `0x5744b8` → RVA `0x1744b8`.
   With ASLR the real addresses are `module_base + RVA`. Read `module_base` from
   x32dbg's Symbols/Memory-Map (the `vcds.exe` entry) or WinDbg `lm m vcds`, then
   compute `iv = module_base + 0x1384d0`. **Verify** you have the right address by
   dumping 16 bytes at `iv` and confirming they equal `IV_TABLE[0]` from
   `link_cipher.py`: `56 51 54 3b 24 45 15 21 03 54 1a 34 54 82 10 4c`. Row 4 (engine
   `f3`) is at `iv + 0x40`: `4d 39 86 a3 de e2 ba 2a d0 4c 1c df 23 34 45 ee`.

---

## 2. The five reference links — honest verdicts (what each does and does NOT buy us)

The recurring theme across all public work: the native x86 tools **UNPACK** (recover a
clean image + OEP + import table) but **do NOT devirtualize** virtualized routines.
Only the hackyboiz Pin+Triton method truly devirtualizes; link 5 is the anti-debug
enabler that makes any live attach (Tier 0/A/B) possible on VMProtect 3.6+.

| link | what it is | verdict for OUR target |
|------|-----------|------------------------|
| 1. `void-stack/VMUnprotect.Dumper` | **.NET-only** VMProtect dumper (MSIL/CLR assemblies) | **NOT applicable.** `VCDS.exe` is native x86-32 Delphi, not .NET. Ignore. |
| 2. `sudha2323/vmprotectunpacker` | native x86/x64 **runtime dump-unpacker**: suspended launch → INT3 at OEP → dump decrypted sections → Capstone | Defeats **packing**, not **virtualization**. A section dump yields VM bytecode + interpreter, not readable KDF code. **Clean image only** — feeds **Tier 0**, not a shortcut to `K`. |
| 3. `hackyboiz` VMPpart2 (Pin + Triton) | **true devirtualization** of native x86 VMProtect: DIE version-detect → Pin instr/bytecode/register trace around the VM entry → cluster handlers → Triton lifts each handler's semantics → opcode→handler→semantics map → reconstruct native (Keystone) & patch `vir_Entry` | **The only true devirt** and the backbone of **Tier B** (§5). |
| 4. `muhammadh772` Delphi-VMProtect unpacking | native x86 **Delphi** VMProtect unpack (our exact profile): bp on `VirtualAlloc`/`VirtualProtect` to catch the unpack → find OEP (`push ebp` in the dumped range, "run to user code"/"execute till return") → dump the allocated region → **Scylla** rebuild imports | **Clean unpacked image only** (same ceiling — does not devirtualize). This is the recipe for **Tier 0**: it gives us a static-analysis-parity unpacked x86 image (we currently only have the arm64-unpacked build). Prerequisite/enabler, not a shortcut to `K`. |
| 5. `cyber.wtf` "Defeating VMProtect's latest tricks" | defeating **VMProtect 3.6+** anti-analysis (kernel-level). Outcome: unpacked, did NOT devirtualize | **The anti-debug checklist** that makes attaching a debugger to this target actually possible. Folded into **Tier A prerequisites** (§ below) and the environment section. Clean image ceiling, but indispensable for getting *in*. |

---

## 3. Static anchors (put in the debugger up front)

| item | arm64 VA (analysable proxy) | old x86 live VA (base 0x400000) | RVA |
|------|-----------------------------|----------------------------------|-----|
| 16-row link-cipher `IV_TABLE` | `0x140171d30` | **`0x5384d0`** (byte-identical, confirmed dump VCDS-2 @file off `0x271646`) | `0x1384d0` |
| 128-byte genuineness/static table | `0x140171730` | **`0x5744b8`** | `0x1744b8` |
| session counter | `0x1405e8108` | *(find x86 equivalent — see §5 tuple)* | — |
| channel selector `(byte+1)&0xf` | `0x140073150` | *(virtualized in x86)* | — |
| AES-encrypt block (arm64 native T-table) | `0x1400780a8` | *(the x86 analogue is the Tier-A target)* | — |

Engine channel: `cid=4`, keystream `KS_F3`, `IV_TABLE[4]` @ live VA `0x538510`.

---

## Tier 0 — produce a clean UNPACKED x86 VCDS.exe image (enabler for A and B)

We currently have an unpacked **arm64** proxy but **no unpacked x86 image**. Tier 0
gets one, giving static-analysis parity on the actual target binary (to place the KDF
VM entry, the x86 session counter, the analogue of the arm64 set-key path, etc.). This
UNPACKS only — it does **not** devirtualize the KDF (same ceiling as links 2/4). It is a
prerequisite/enabler, not a shortcut to `K`.

Recipe (link 4, Delphi-VMProtect profile):

1. Launch `VCDS.exe` under x32dbg with ScyllaHide (VMProtect profile) — see Tier A
   prerequisites below for attaching past the anti-debug.
2. Breakpoint `kernel32!VirtualAlloc` / `kernel32!VirtualProtect` (or their `Nt*`
   equivalents) to catch VMP allocating/unpacking the real code into fresh memory.
3. Find the OEP: after the unpack, look for the Delphi prologue (`push ebp; mov ebp,esp`)
   in the newly-allocated range; use "run to user code" / "execute till return" to stop
   at it.
4. Dump the unpacked region (Scylla / x32dbg dump), then **Scylla** → rebuild the import
   table (IAT autosearch → get imports → fix dump).
5. Load the rebuilt image in IDA/Ghidra. Use it to locate: the KDF VM entry (Tier B §B.1),
   the x86 session-counter (arm64 `0x1405e8108` analogue, §B.6), and to confirm the
   `IV_TABLE`/static-table live VAs.

Deliverable: `VCDS-x86-unpacked.exe` — parity with `VCDS-arm64-unpacked.exe`.

---

## Tier A — hardware-breakpoint the native AES keystream generator (cheap, try FIRST)

### A.1 Hypothesis

Vendors virtualize the **KDF** (derive-`K_epoch`, license) but almost never the
**bulk per-frame AES** — virtualizing per-frame crypto is a ~100× slowdown they
avoid. VMProtect virtualizes *marked* functions; the rest unpacks to native at
runtime (which is exactly why DIE reports high static entropy — that is the packed
image at rest, not necessarily the runtime shape). So per-frame
`KS_cid = AES(K).enc(IV_TABLE[cid])` is very plausibly native, and the moment it runs
it **reads a row of `IV_TABLE`** as the plaintext block. Break on that read and you
land inside the encrypt routine with the key/round-key pointer live in registers or
on the stack — no contiguous heap blob required.

### A.2 Three sub-targets, best first

1. **CNG / BCrypt (strongest, and native by definition).** The process links
   `bcryptprimitives.dll` and touches `\Device\CNG` (per `vcds-rus-crack-findings.md`).
   If VCDS does AES via CNG, the **key is imported in the clear** through
   `BCryptGenerateSymmetricKey` / `BCryptImportKey` / `BCryptImportKeyPair`, and every
   block goes through `BCryptEncrypt`. CNG is an OS DLL — **never virtualized**. This
   is the single cleanest win and also explains the FIPS-197 scan negative (CNG stores
   the expanded key inside an opaque `BCRYPT_KEY` object, not a 240-byte heap array).
   Breakpoints:
   - `bcryptprimitives!BCryptGenerateSymmetricKey` — arg `pbSecret`/`cbSecret` = the
     **raw 32-byte `K`** (this is the prize if it fires).
   - `bcrypt!BCryptEncrypt` — `hKey` handle; walk it to the key bytes, or just capture
     `pbInput` (should equal an `IV_TABLE` row) + `pbOutput` (the keystream) to confirm
     the relation even without `K`.
2. **`IV_TABLE`-read HW breakpoint (works regardless of AES backend).** Set a memory
   **read** DR breakpoint on the engine row `IV_TABLE[4]` (live VA `0x538510`, or
   `module_base + 0x1384d0 + 0x40`). It fires at the exact instruction consuming the
   IV as the AES plaintext. Inspect the caller:
   - **Native AES** (S-box / T-table lookups, `xor`/`rol`, a 240-byte round-key window
     reachable through a register/stack pointer) → dump the round keys, or set a second
     read-bp on the round-key buffer and grab the 32-byte master `K` at schedule time.
   - **VM dispatch** (a `fetch → switch/jump-table → handler` interpreter loop reading
     a bytecode pointer) → AES is *also* virtualized → **fall through to Tier B**. This
     outcome is not a wasted run: it *diagnoses* that the crypto is virtualized, which
     is the exact precondition Tier B needs.
3. **Native key-schedule buffer (if 1 gives nothing and 2 shows native AES).** Once
   inside a native AES-256 encrypt, the round-key pointer is a register/stack arg.
   240 bytes of AES-256 round keys → the first 32 are the master `K`. Read them at the
   hit and feed to the oracle.

### Tier A prerequisites — get a debugger attached to VMProtect 3.6+ (link 5)

VMProtect 3.6+ goes well beyond the classic API checks. The concrete checklist from
`cyber.wtf` (link 5), what to harden:

- **Direct syscalls (sysenter/`syscall`) that bypass user-mode API hooks.** VMP calls
  into the kernel without going through `ntdll` stubs, so hooking `Nt*` in user mode is
  not enough — the hook must know the target Windows build's **syscall table/numbers**.
  ScyllaHide (a build that covers direct-syscall techniques) and pinning to a known OS
  build are the mitigation.
- **Hypervisor + emulation detection** (see env §1.0): CPUID hypervisor leaf, Trap-Flag /
  RDTSC timing, SMBIOS & BIOS Option-ROM firmware tables. Native bare-metal x86 avoids
  all of it; a hardened VirtualBox ≥ 7.0.4 is the fallback.
- **`KUSER_SHARED_DATA` access** used to sanity-check timing / debugger state — HW
  breakpoints that touch it can be observed; keep breakpoints on the crypto path, not on
  shared-data reads.
- **API hooks to cover:** `NtQueryVirtualMemory`, `NtOpenFile`, `NtCreateSection`,
  `NtMapViewOfSection` (in addition to the classic `NtQueryInformationProcess`
  `ProcessDebugPort/Flags/Object`, `NtSetInformationThread` `HideFromDebugger`,
  `IsDebuggerPresent`). Use a ScyllaHide build modified to cover multiple techniques.
- **Avoid Heaven's Gate** (WOW64 x86-on-x64 transitions) — run a native 32-bit context
  for the 32-bit `VCDS.exe` where possible (link 5 did exactly this).

This is substantial (kernel-level) work; budget for it before expecting a clean attach.

### A.3 Attaching past VMProtect's anti-debug (debugger-side tactics)

On top of the prerequisites above:

- **ScyllaHide** (x64dbg plugin): enable the VMProtect profile — hooks the detection
  APIs so the attach and single-steps stay hidden. Turn it on **before** launching the
  target under the debugger.
- **Hardware DR breakpoints** (used by both scripts below) are stealthier than INT3
  software breakpoints: they don't modify code bytes, so VMP's code-checksum/self-CRC
  checks don't trip. Risk: some VMP builds read DR7 to detect HW breakpoints —
  ScyllaHide's "protect DRx" / drx-spoof option counters this.
- **Attach vs. launch.** Prefer launch-suspended-then-arm so breakpoints are set
  before VMP's early anti-debug runs. In x32dbg: open the exe (it breaks at the system
  breakpoint / EP) with ScyllaHide active, then run the script. If the crack requires
  `VCDSLoader` to inject `vcds_hook.dll` first, attach to the already-running `VCDS.exe`
  child instead, with ScyllaHide's "attach" hardening on.

### A.4 x64dbg / x32dbg recipe

1. Launch `x32dbg`, enable ScyllaHide (VMProtect profile).
2. Open `VCDS.exe` (or attach to the running child if loader-injected).
3. Run `research/clb-crack/hwbp_ivtable.x64dbg.txt` (rebases, verifies `IV_TABLE`,
   arms the HW read-bp on row 4, and also drops BCrypt breakpoints).
4. In VCDS, open Engine 01 → trigger a read (§1.3). The bp fires.
5. At the hit: `Follow in Disassembler` on EIP. Native AES vs VM dispatch (A.2). If
   native, the script prints the register file + a stack window; locate the round-key
   pointer, dump 32 bytes, that is `K` (or dump 240 and take the first 32).
6. Feed to the oracle (§0).

### A.5 WinDbg recipe

1. `windbg -g VCDS.exe` (or `-p <pid>` to attach). Load a symbol path for
   `bcryptprimitives`/`bcrypt` (Microsoft symbol server) so the CNG names resolve.
2. Run `research/clb-crack/hwbp_ivtable.windbg.txt` (command script) — it computes the
   rebased address, verifies the table bytes, sets `ba r` on the IV row and on
   `BCryptGenerateSymmetricKey`/`BCryptEncrypt`, and installs a JS handler that dumps
   registers/stack and searches nearby memory for the 32-byte key.
3. Trigger the engine read; inspect the break per A.2; feed `K` to the oracle.

### A.6 Feed the result to the oracle

Whatever you recover (32-byte `K`, or a 240-byte schedule → first 32 bytes), run §0.
`KS_F3` match or UDS decodes = done. If the break landed in a VM interpreter loop
(no native AES, no CNG call) → AES is virtualized → **Tier B**.

---

## Tier B — full devirtualization of the KDF (heavy; only if Tier A shows virtualization)

Follows the hackyboiz Pin + Triton pipeline. Do this only if Tier A proves the crypto
(or at least the KDF) is virtualized and we need the *derivation* itself.

### B.0 Detect the VMProtect version

Run **Detect It Easy** on `VCDS.exe`: note the VMProtect version and the `.vmpN`
section name(s) / entropy. Confirm virtualization mode (vs. pure mutation). Optionally
run `sudha2323/vmprotectunpacker` to get a cleaner unpacked image and the OEP — this
does **not** deobfuscate the VM, it just gives Pin a tidier target and confirms where
native execution begins.

### B.1 Find the VM entry for the KDF

Statically (IDA/Ghidra on the unpacked image) trace the open sequence to the KDF:
the code that runs between the `b6` frame going out and the first `KS_cid` encrypt.
On the arm64 proxy the chain is dispatcher `0x14006d6c8` → set-key `0x140072ec0`; the
x86 KDF sits in the analogous session-setup region and pushes into the VM. Mark the VM
entry address (the `push`/`call` into the interpreter) — this is the pintool's arm point.

### B.2 Pin trace (custom pintool → 3 artifacts)

`research/clb-crack/kdf_trace.pin.cpp` (skeleton provided) instruments the `.vmp`
range and, once EIP enters the VM entry, logs:

- `vmtrace.out` — every executed instruction (address + machine bytes) from VM-entry
  to VM-exit.
- `bytecode_values.txt` — the operand words read from the VM bytecode pointer
  (`[ESI]`-style) at each handler entry.
- `handler_registers.txt` — a register snapshot (EAX..EDI, ESP, EFLAGS) at each
  handler entry.

Build (MSVC x86): `make TARGET=ia32` in the pintool dir, then
`pin -t kdf_trace.dll -- VCDS.exe`. Trigger one engine-open (§1.3) so exactly one KDF
runs, then detach.

### B.3 Handler identification & clustering

- `uniq -c` / frequency-rank `vmtrace.out`: the highest-frequency address is the VM
  dispatcher; the next tier are the handlers.
- Segment the trace into per-handler blocks (glue + body). Cluster segments with
  identical byte sequences → pick one canonical representative per handler.

### B.4 Triton — lift each handler's semantics

`research/clb-crack/kdf_triton.py` (harness outline provided) loads a canonical handler
segment + its entry register snapshot, executes it symbolically, and diffs pre/post
register + stack state to name the handler (e.g. `LCONST`, `ADD`, `XOR`, `ROL`, memory
load/store, and the bytecode-pointer advance). hackyboiz recovered constant-decrypt
chains like:

```
val = (enc + 0x55106798) & 0xFFFFFFFF
val = (-val) & 0xFFFFFFFF
val = (val + 0x69733a52) & 0xFFFFFFFF
val = ((val << 1) | (val >> 31)) & 0xFFFFFFFF
dec = ~val & 0xFFFFFFFF
```

Build the `opcode → handler → semantics` map by cross-referencing the dispatcher's
fetch (logged in `bytecode_values.txt`) with `vmtrace.out`.

### B.5 Reconstruct the KDF & reimplement in Rust

Walk the VM bytecode, emit the native-equivalent semantics per opcode, and read off
the algorithm as pseudo-code. (hackyboiz then assemble with Keystone and patch
`vir_Entry`; **we don't need to patch** — we only need the *algorithm* so vagcan can
compute `K_epoch` itself.) Tie it to our tuple model:

```
K_epoch = KDF( advancing_session_counter , static_table@0x5744b8 , b6/b7 nonce )
```

The devirt goal is to confirm/replace this with the exact function: which inputs, in
what order, through which mixing (the recovered handler chain). Then reimplement as a
Rust module (future `crates/vag-hex/src/kdf.rs`) that, given the session counter + the
embedded 128-byte table + the `b6` nonce, outputs `K_epoch`; validate each candidate
via the same oracle (§0), then via full live decode.

### B.6 Locate the x86 session counter

The arm64 counter is `@0x1405e8108`. Find the x86 equivalent by (a) the Pin trace —
whichever monotonic per-epoch value the KDF reads that is neither the static table nor
the `b6` nonce — and (b) a HW write-bp hunt: after a known open, diff a memory region
that increments once per epoch. Feed its live VA into the Rust reimplementation.

### B.7 Deliverables the owner runs on the x86 box

- `research/clb-crack/kdf_trace.pin.cpp` — pintool (build with the Pin MSVC ia32 kit).
- `research/clb-crack/kdf_triton.py` — Triton lifting harness.
- Recovered `opcode→semantics` table + KDF pseudo-code (writeup output).
- A future `crates/vag-hex/src/kdf.rs` implementing the recovered KDF, gated by the §0 oracle.

---

## 4. Risk / effort per tier

| tier | effort | success probability | key risk | payoff |
|------|--------|--------------------|----------|--------|
| **0 — clean unpacked x86 image** | ~hours–day | high (well-trodden, link 4) | anti-debug attach cost (link 5) | static-analysis parity → places KDF VM-entry & counter for A/B; NOT `K` itself |
| **A1 — CNG BCrypt bp** | ~hours | high **if** AES uses CNG | VCDS may use in-house LibTomCrypt AES, not CNG → no BCrypt hit | raw 32-byte `K` straight from `BCryptGenerateSymmetricKey` |
| **A2 — IV_TABLE read HW-bp** | ~hours | medium-high | AES may be virtualized → bp lands in VM loop (but this *diagnoses* it) | round-key pointer / `K` at the encrypt site; or a definitive "it's virtualized" verdict |
| **A3 — native schedule dump** | ~hours | medium | needs A2 to show native AES first | first 32 bytes of the 240-byte schedule = `K` |
| **B — full devirt of KDF** | days–weeks | medium (proven technique, but labor-heavy) | handler zoo size; anti-debug during long traces; correctly isolating the KDF VM-entry | the *algorithm* → vagcan computes any epoch's `K` offline forever |

Order: A1 and A2 in the **same** debugger session (set all breakpoints at once), then
A3 if A2 shows native AES. Only escalate to B if A2/A1 prove virtualization.

---

## 5. Honest confidence: does Tier A (native AES HW-bp) work, or is the AES virtualized too?

**My estimate: ~55–65% that Tier A yields `K` directly.**

Arguments **for** Tier A working:
- Per-frame bulk AES is the classic thing vendors leave native (the ~100× VM slowdown).
  VMProtect virtualizes *selected* functions; a whole-binary entropy-8 reading is
  consistent with **packing at rest**, not with every function being VM'd at runtime.
- The process links CNG (`bcryptprimitives.dll`). If AES routes through CNG, the key
  import (`BCryptGenerateSymmetricKey`) is in an OS DLL that is **never** virtualized —
  a clean, high-value break. This *also* explains the FIPS-197 scan finding nothing
  (CNG holds the expanded key in an opaque object, not a 240-byte heap array).
- A live HW-bp follows the pointer the code uses, so it works even if the round keys
  live in a VMProtect-guarded region a static minidump never surfaced. The frozen-dump
  negative is weakly informative about the live case.

Arguments **against** (why it might be virtualized):
- The x86 `.text` sections read entropy 8.00 with no unpacked region in the static
  image; if the per-frame AES-ECB(row) turns out to be inside the VM too, the IV-read
  bp fires inside interpreted handlers and there is no clean native round-key pointer.
- The prior 5-dump scan found **zero** schedules of any size/layout. Weak evidence
  live (snapshot vs. pointer-follow), but non-zero — a fully native LibTomCrypt AES
  writing a 240-byte context to the heap would plausibly have been caught in *some*
  snapshot, and it was not. That nudges toward "not a plain native heap schedule"
  (either CNG-opaque, or virtualized).

**Bottom line:** Tier A is clearly worth trying first — it is cheap (hours), and even
a "failure" is a *diagnosis*: if the IV-read bp lands in a VM dispatch loop and no
BCrypt call ever fires, that is the definitive signal that the crypto is virtualized
and Tier B is required. The CNG breakpoint is the highest-expected-value single action
in the whole playbook; run it first.

---

## First command the owner runs on the x86 host (Tier A)

After launching `VCDS.exe` under **x32dbg** with **ScyllaHide** (VMProtect profile)
active, paste-run the script:

```
scriptload research\clb-crack\hwbp_ivtable.x64dbg.txt
```

Then open Engine 01 in VCDS and trigger a data read (§1.3). When the breakpoint fires,
inspect EIP per §A.2, dump the 32-byte key, and validate with §0.

(WinDbg alternative: `$$><research\clb-crack\hwbp_ivtable.windbg.txt`.)
