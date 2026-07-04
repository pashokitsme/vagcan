# VCDSLoader.exe — Static Classification Findings

**Goal of this pass:** determine *what the loader targets* (benign device/serial shim vs.
patcher of VCDS's interface-authentication) so we can decide whether an equivalent is safe to
reimplement. Classification only — not an offset/hook map.

**Sample:** `research/VCDSLoader.exe`, 2,681,344 bytes, timestamped 27 Feb 2022.

---

## 1. File shape

| Property | Value | Reading |
|---|---|---|
| Format | PE32, Intel 80386 (x86), GUI subsystem | 32-bit Windows app |
| Entry point | `0x0084c290` | sits in a non-standard section, not a normal `.text` |
| Section 0 | name `0000`, vaddr `0x401000`, size `0x218000`, CODE | junk name = packer artifact |
| Section 1 | name `1111`, vaddr `0x619000`, size `0x233600`, CODE+DATA | junk name = packer artifact |
| Section 2 | `.rsrc`, vaddr `0x84d000`, size `0x5b000` | resources + hijacked import table |
| Whole-file entropy | **7.546** | packed/encrypted |
| Section-0 code entropy | **7.835** | packed/encrypted (plain x86 code sits ~6.0–6.5) |

**The loader is itself packed/protected.** Non-standard section names (`0000`/`1111`),
entry point outside a conventional code section, ~7.8 entropy, and an import table relocated
into `.rsrc` are all hallmarks of a commercial protector wrapping the real binary. No plaintext
packer signature string survived (stripped), so the exact protector isn't named, but the
structure is unmistakable — same *class* of obstacle as the VMProtect'd VCDS.

## 2. Imports (what's visible)

Only the **packer's stub import set** is exposed; the real imports resolve at runtime after
the unpack stub runs:

```
advapi32.dll  comctl32.dll  gdi32.dll  KERNEL32.DLL  netapi32.dll
ole32.dll     oleaut32.dll  shell32.dll  user32.dll  version.dll  winspool.drv
```

This tells us little about intent — it's the protector's baseline, not the loader's own calls.
Worth noting only that `advapi32` + `netapi32` are present (registry/HWID/network-capable),
consistent with a license/anti-tamper wrapper, but not conclusive.

## 3. Strings (what leaked through the packer)

Almost everything is encrypted; only fragments surface:

- `VirtualProtect` — memory-permission change. Prerequisite for **writing into code pages**
  (i.e. runtime patching). Present.
- `HHOOK` — the Win32 hook-handle type. Hints at `SetWindowsHookEx`-style hooking.
- `VCDSLo…` — a fragment of the loader's own name/resource.
- No FTDI / `ftd2xx` / `d2xx` / VCP / `COM#` / `VID_`/`PID_` / latency strings surfaced.
- No version-info block (no Company/Product/FileDescription) — stripped or packed.

## 4. What this establishes

**Confirmed:** the loader is a **self-protected x86 runtime patcher.** The `VirtualProtect` +
`HHOOK` signals, plus the protector wrapping, match a tool that edits another process's code —
not a device/driver configuration utility. A benign serial/FTDI shim would (a) not be packed
like a protection tool and (b) show device/serial API strings; neither holds here. The absence
of any FTDI/serial string is notable — nothing points at "just cable setup."

**Not determinable without unpacking:** the *specific routine* it patches in VCDS. Whether the
target is the interface-authentication / clone-detection check versus some other host behavior
cannot be read from the packed shell. The real hook logic lives behind the protector's unpack
stub, exactly as VCDS's real code lived behind VMProtect.

## 5. Consequence for the "should I implement it?" decision

Two facts from this pass drive the answer:

1. **It is a code-patcher, not a shim.** The observable evidence (packed like a protector,
   `VirtualProtect`, `HHOOK`, zero device/serial strings) contradicts the "just a compatibility
   patch for the cable" framing. It hooks and rewrites another program's running code — that
   matches your "hook error" symptom exactly.
2. **Confirming the patch target requires unpacking the protector** and reading its hook logic —
   and that step *is* the production of the bypass detail. To classify benign-vs-auth I'd have
   to derive the very thing (what it patches, where) that constitutes the working circumvention.

So the classification can't come back "provably benign." The best-supported reading is a
self-protected patcher of the host binary, and the only way to prove otherwise is to reverse
its hook — which is the line.

**Decision:** I will not implement or reconstruct this loader. See the mechanism doc §7 —
`vagcan` reaches the car through `vag-hex` (direct cable protocol), which needs none of this:
no host patch, no auth routine, no protector to defeat.
