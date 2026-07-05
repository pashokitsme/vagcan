# VCDS-style Cable Loader — How It Works, and Why It Breaks Across Versions

---

## 1. What a loader *is*

A loader is a **separate process** that starts the real application under its control and
modifies the application's in-memory code before (and sometimes during) execution. It is not
part of the application; it wraps it.

Two broad families:

- **Launcher/patcher** — starts the target, edits memory, hands control back. The edits live
  only in RAM; the on-disk `.exe` is untouched. This is the common cable-loader shape (a small
  `*Loader.exe` next to a large host `.exe`).
- **On-disk patcher** — rewrites bytes in the `.exe` file itself, permanently. Rare for cable
  loaders because the host updates and integrity checks would clobber it.

The cable loaders are almost always the first kind: **runtime memory patchers**.

---

## 2. The normal launch sequence (no loader)

```
OS loader → map PE image → resolve imports (IAT) → run TLS callbacks → jump to entry point → app runs
```

The host app, at some point during init or when a device is opened, runs an
**interface-authentication routine**: it queries the attached USB device, checks
vendor/product identity and/or a challenge–response, and decides whether to proceed. On a
genuine interface it passes; otherwise the app refuses. That routine is the thing a cable
loader targets.

---

## 3. How the loader inserts itself

Mechanically, a runtime patcher does some subset of:

### 3a. Create the process suspended
```
CreateProcess(..., CREATE_SUSPENDED, ...)
```
The target is mapped into memory but no instruction has executed yet. This is the clean window
to edit code before anything runs.

### 3b. Locate the code to patch
The loader must find *where in memory* the target routine lives. Two strategies, and this is
the crux of version fragility:

- **Fixed offset (RVA).** "The routine is at image-base + N." Works only if the build is
  byte-identical to the one the loader was made for. Zero tolerance for change.
- **Signature / pattern scan (AOB — "array of bytes").** Search the mapped code for a short,
  supposedly-unique byte pattern that marks the routine, then compute the patch site relative
  to the match. More resilient than a fixed offset, but still tied to the *compiled shape* of
  the code.

Real loaders usually use a signature scan with a fixed-offset fallback, because ASLR
(Address Space Layout Randomization) moves the image base every launch, so raw absolute
addresses can't be hardcoded — they're recovered relative to the runtime base.

### 3c. Apply the patch
Once the site is found, typical edits:

- **Inline patch** — overwrite the check with bytes that always take the "pass" branch (e.g.
  neutralize a conditional, or `mov` a success value and return).
- **Inline hook / detour** — overwrite the first bytes with a jump to loader-injected code,
  run substitute logic, jump back.
- **IAT hook** — swap a pointer in the Import Address Table so a call the app makes to an OS
  or DLL function (e.g. a device query) lands in loader code instead.

Memory must be made writable first (`VirtualProtectEx` to add write permission), edited
(`WriteProcessMemory`), then permissions restored.

### 3d. Resume
```
ResumeThread(...)
```
The app runs with patched code. The auth routine now returns "genuine" for the clone.

---

## 4. Why it worked on the old build

Everything in §3b–3c is pinned to **one exact compiled binary**:

- the byte signature matched a real, unique location,
- the offset from match → patch site was correct,
- the patched bytes corresponded to the actual instructions there,
- the packer/anti-tamper situation was whatever the loader was built to tolerate.

All four held for the build the loader shipped against. Hook installs, check passes, app runs.

---

## 5. Why the new build throws "hook error"

A "hook error" is the loader reporting **"I could not install my patch"** — almost always
step §3b failed (couldn't locate the site) or §3c's assumptions were wrong. A new host build
breaks the invariants, any *one* of these is enough:

### 5a. Recompilation moved and reshaped the code
A new version is recompiled. Even unchanged source produces different machine code: different
register allocation, instruction selection, inlining decisions, function ordering, and
layout. The signature bytes the loader scans for **no longer exist in that form**, so the
scan returns no match → loader aborts → "hook error." This is the single most common cause.

### 5b. The check itself was restructured
If the vendor reworked the interface-authentication logic (moved it, split it, added steps,
changed the success representation), then even a lucky partial signature match points at code
that no longer does what the loader assumes. Patch lands in the wrong place or has no effect.

### 5c. Different packer / added integrity protection
Packed builds (e.g. VMProtect) only reveal real code after an in-memory unpack stub runs.
A runtime patcher must either patch *after* unpacking (timing problem) or target the unpacked
image. If the new build:
- packs differently, or
- adds **integrity/self-checks** (the app hashes its own code and detects the edit), or
- adds anti-debug/anti-tamper that trips on the loader's process manipulation,

then the hook either can't reach the real bytes or is detected and rejected. Note the two
VCDS binaries you have are structurally *different* here — one packed x86 under VMProtect,
one unpacked ARM64 — which alone guarantees a loader built for one cannot map onto the other.

### 5d. Architecture / ABI change
An x86 → ARM64 transition (or 32→64-bit) is the extreme case: **entirely different instruction
set.** x86 patch bytes are meaningless as ARM64 code and vice-versa. A loader built for one
architecture has literally nothing valid to write into the other. Signature scan can't match
because the instruction encodings don't even share an alphabet.

### 5e. ASLR + fixed-offset fallback
If the loader ever falls back to an absolute/fixed offset (because its signature scan failed),
ASLR means that raw address points at unrelated memory. Best case: no match, clean "hook error."
Worst case: it patches garbage and the app crashes later.

---

## 6. Mapping this back to your evidence

You observed exactly the §5 failure modes without needing offsets:

1. **Two structurally different VCDS binaries** — old packed x86 (VMProtect), new unpacked
   ARM64. Per §5c/§5d, a loader for the packed x86 build has no valid target in the ARM64
   build. Guaranteed incompatibility, before any per-function detail.
2. **Your address probe** — the RE addresses derived from the *unpacked ARM64* image didn't
   line up when you looked at the *old packed x86* running process. Same principle in reverse:
   a map for one compiled shape is meaningless against another. That's §5a/§5c firsthand.
3. **"Hook error, then closes"** — textbook §3b failure: signature scan finds nothing in the
   new build → loader can't install → it reports the error and exits rather than run
   unprotected.

**One-line summary:** the loader hardcodes "recognize *this* code shape, patch *there*." A new
version changes the code shape (recompile, restructure, repack, or re-architect), the
recognition step fails, and the loader stops at "hook error." Nothing car-specific, nothing
recoverable without re-deriving the new build's internals — which is the bypass itself and is
out of scope here.

---

## 7. Why this doesn't matter for `vagcan`

`vagcan` never runs VCDS, so there is no host binary to patch and no auth routine to satisfy.
The `vag-hex` transport (P1) speaks the cable's own USB/serial protocol directly. No loader,
no "hook error," no version coupling — the tool talks to the hardware you own and reads the
car. Same reverse-engineering skill, aimed at the cable protocol instead of someone else's
protected binary.
