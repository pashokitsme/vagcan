# VCDS `.clb` / `.rod` crack — FINDINGS

Status: **CRACKED** — container format fully cracked; legacy cipher fully cracked;
modern (MQB) cipher **fully recovered** from the UNPACKED ARM64 build
(`bin/VCDS-arm64-unpacked.exe`). The modern cipher is **TEA** (64-bit block, 32
rounds, delta 0x9E3779B9, LE words) in **CBC** with two hardcoded keys
(KEY_CLB / KEY_ROD) and a per-record derived IV. `.clb` decodes cleanly and
completely; `.rod` blocks 2..n decode exactly (first-block IV is a minor gap).
See **NOTES-modern.txt** for the algorithm, keys, binary addresses (VMAs) and
validation, and **decrypt_modern.py** for the working decryptor. The sections
below (3-5) describe the earlier, now-superseded "key not extracted" state.

## 1. Container format — FULLY CRACKED (high confidence)
Working parsers in `decoder.py`, validated on all 23 samples.

- **`.clb`** = flat sequence of records: `00 <len>` (2-byte big-endian **plaintext**
  length) + ciphertext padded up to a multiple of 8 bytes + `00 0a` terminator.
  A bare `00 0a` is a blank line. (e.g. len=43 → 48 cipher bytes.)
- **`.rod`** = plaintext ODX section framing `[CMP]\r\n…\r\n[/CMP]` etc. Tags seen:
  CMP ADP DTC IDN INC GES MWB SLV SOT XPL. `[MWB]` (measurements) uses the same
  `00 <len> … 00 0a` records as `.clb`; `[CMP]`/`[ADP]`/… use a 5-byte header
  `80 00 <len> 00 00` where `<len>` is the ciphertext byte count (a multiple of 8),
  then the blocks. **One cipher underlies both formats.**

## 2. Legacy `.clb` cipher — FULLY CRACKED (high confidence)
Recovered from the public **SVCdec** PHP tool (saved at `ref/SVCdec.php`), independently
re-implemented and round-trip-validated in `decoder.py` (`_selftest`).

- Keystream = 1597-byte table `keycode[f] = int(key[f]/((f&3)+2)/((f&5)+1)) % 256`,
  where `key` is the Ross-Tech **"How to Copy and Paste"** English help text.
- Per byte: `cch = keycode[pos] | 0x80`; decode `P = cch ^ ((C - cch) & 0xFF)`;
  encode `C = ((cch ^ P) + cch) & 0xFF`;
  `pos = ((lineNum % 16) * p + z) % 256 + byteIndex`,
  presets: method1 `p=3,z=250`, method2 `p=2,z=250`, method3 `p=3,z=233`.
- NOTE: none of our MQB samples are legacy — this cipher is for older label files.

## 3. Modern / MQB cipher — IDENTIFIED, KEY NOT RECOVERED (high confidence on shape)
All provided samples (incl. every `.rod`) use this. `decoder.py` extracts the exact
8-byte cipher blocks, ready for a key.

- **64-bit block cipher** — padded lengths are multiples of 8 that are NOT multiples of
  16 (8, 56, 88), ruling out AES.
- **Fixed key, deterministic** — identical plaintext → identical 8 cipher bytes across
  unrelated files (trailer `58 a2 ed c5 28 4d d4 d7` shared by `4T-02.clb` and
  `08E-907-554.clb`; `-`-family header block `e6 42 7c 2e 26 10 07 a1` is block 0 of all
  five `-`-family files).
- **CBC with a fixed per-record IV** — among record pairs sharing block 0, the common
  ciphertext prefix is always a multiple of 8 (328/330 diverge at exactly byte 8);
  interior blocks 98.7% unique; recurring blocks always share the full preceding prefix.
  A stream cipher would diverge at random offsets.
- **Ruled out:** single-byte/repeating XOR; the legacy keystream (every offset × 7
  transforms → garbage); per-position substitution (per-`index%16` column freq analysis
  → garbage); all compression (no zip/gz/bz2/xz/zstd/lz4 magics; raw-inflate at every
  offset failed); TEA/XTEA with guessable keys. Likely a legacy Delphi 64-bit cipher
  (DES/3DES/Blowfish/XTEA/CAST/IDEA) — not narrowable without the key.

## 4. Why the key is blocked — the RE wall
`bin/VCDS.exe` is **packed** (VMProtect/Themida-class): every section at entropy 8.00
with blank names, `AddressOfEntryPoint` RVA `0x101aa2c4` outside the image, a section
with ~259 MB virtual size, no crypto constants findable statically, the legacy key text
absent as a plaintext string. `RT-USB.dll` is unpacked but is USB transport only.
→ Static extraction with objdump alone is not possible.

**Precise lead:** SVCdec's commented-out disassembly preserves the VCDS fragment
`v5 = (((v4[158854]) + v11) & 0xFF) + 2 * ((v4 + 20) & 0xF);` — `158854 = 0x26C46` is the
key-table offset in the (unpacked) VCDS.exe they analyzed; `2*((idx+20)&0xF)` is the
period-16 line-position term. That data region holds the keystream/block key.

## 5. To finish the modern crack (needs a Windows dynamic-RE session)
Unpack `VCDS.exe` dynamically (x64dbg/Scylla or an emulator, run to OEP), breakpoint the
`.clb`/`.rod` file read, identify the 64-bit block cipher's round function / S-boxes, dump
the fixed key + IV, then plug into `decoder.py::block_decrypt` (CBC, IV reset per record).
The parsers already hand over the exact 8-byte blocks.

Alternative to try first (cheaper): check open-source projects for a published modern key
— VAG-Looker, PyVCDS, and forks — before doing the dynamic unpack.

## Deliverables (in `research/clb-crack/`, gitignored)
- `decoder.py` — `.clb`/`.rod` container parsers + working legacy decoder (self-test) +
  documented `block_decrypt()` hook for the modern cipher.
- `ref/SVCdec.php` — reference legacy decoder, verbatim.
- `samples_decoded_structure.txt` — container-layer decode showing fixed-IV shared-prefix
  behavior across a label family.

Sources: VAG-Looker (github klosik007), SVCdec (github isublimity), PyVCDS
(github baconwaifu), ross-tech.com/vag-com/labels.php.
