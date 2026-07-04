# .clb / .rod format crack — intel brief

Goal: recover how Ross-Tech VCDS decodes its compiled label containers, produce a
working decoder, and validate it by decoding sample files to readable measurement
text. This unlocks the modern (MQB) measurement/DTC/adaptation definitions the
`vag-data` crate cannot currently read.

## Two related container formats

### `.clb` (legacy compiled labels)
- Sibling of the plaintext `.lbl` files (already parsed by `crates/vag-data`).
- Binary. First byte `0x00`; byte 1 looks like a length/count. Bytes 2..~10 are a
  per-"family" constant signature, then a high-entropy body.
- Observed first-16-bytes (note the shared middle run within a family):
  ```
  ---08.clb            00 32 e6 42 7c 2e 26 10 07 a1 bd b5 41 30 ae 59
  00-03.clb            00 33 e6 42 7c 2e 26 10 07 a1 40 3c 82 10 29 98
  02-01.clb            00 2e e6 42 7c 2e 26 10 07 a1 f0 a8 ab cc e3 3a
  03-01.clb            00 2d e6 42 7c 2e 26 10 07 a1 18 31 e5 e4 c3 9b
  066-906-032-AQN.clb  00 0d 7e 7c a2 3c 4a 7f 33 c5 9c 0d 41 5b e8 78
  08E-907-554.clb      00 0b 75 75 a1 dd 87 75 09 a0 e7 e9 73 bd 61 df
  ```
- Body entropy: small files ~5.5 bits/byte, large files 7.6–7.9 bits/byte
  (=compressed and/or encrypted, NOT plain XOR of ASCII).

### `.rod` (UDS/ODX data — the MQB-era ECUs, incl. the target 2017 Octavia 1.8 TSI)
- Located in `UDS_EV/` in the install (15,465 files). THIS is the higher-value
  target for modern cars.
- **Plaintext ASCII section framing** wrapping encrypted blocks:
  `[CMP]…[/CMP]` `[ADP]…[/ADP]` `[DTC]…[/DTC]` `[IDN]…[/IDN]`
  (CMP = measurements, ADP = adaptations, DTC = fault codes, IDN = identification).
  Markers are CRLF-terminated: `[CMP]\r\n <block> \r\n[/CMP]\r\n`.
- Bytes immediately after the first `[CMP]\r\n` (note the constant `80 00 <len> 00 00`
  prefix; len−next byte differs by a constant 7):
  ```
  CRFT1_EPH.rod          80 00 28 00 00 21 2c 73 af 3f 7a 07 ed d0 3d b7
  CRFT1_ESP.rod          80 00 28 00 00 21 87 c7 a0 d8 cb 42 cc 00 11 fe
  CRFT1_ESP_3_12288.rod  80 00 30 00 00 29 b0 82 7d 5e 05 6a ba c5 84 79
  CRFT1_ESP_3_12291.rod  80 00 30 00 00 29 00 f5 69 bd db b3 47 e7 da 99
  ```
- The `[CMP]`/`[ADP]`/`[DTC]`/`[IDN]` ASCII markers are KNOWN-PLAINTEXT at known
  offsets, and each block has a small structured header (`80 00 len 00 00 …`)
  before the high-entropy payload — excellent cribs.

## What has been ruled out
- Simple fixed single-byte XOR: no. Body entropy stays ~7.9 after any single-byte
  guess.
- "It's just XOR'd ASCII text": no — high body entropy means there is compression
  or a real cipher under any XOR layer, so naive crib-drag of label words alone
  will likely not reveal readable text directly.

## Files provided (stable local copies — the original install is a removable
`/Volumes/[C] Windows 11.hidden/...` mount with glob-hostile brackets)
- `samples/clb/*.clb` — 14 files spanning tiny→large, incl. same-family header groups.
- `samples/rod/*.rod` — 9 UDS files with CMP/ADP/DTC/IDN sections.
- `samples/4T-02.lbl` — the ONE part number present as both `.lbl` and `.clb`
  (weak known-plaintext lead; note the two hold different content/sizes).
- `bin/VCDS.exe` — the main VCDS loader (PE32 x86). **Contains the decode routine.**
  This is the authoritative RE target.
- `bin/RT-USB.dll` — Ross-Tech USB layer (probably irrelevant to decode, included
  for completeness).

## Suggested attack order
1. **Cheap cryptanalysis first.** Look for a fixed keystream: XOR same-family `.clb`
   files and same-`[CMP]`-header `.rod` blocks pairwise; test repeating-key XOR
   (Vigenère) periods; check whether the `80 00 len 00 00` header decodes a plaintext
   length that matches the block size (→ tells you the header is NOT encrypted and
   where ciphertext begins). Test whether the body is DEFLATE/zlib/LZ (try
   `zlib.decompress` with/without a leading XOR, raw inflate, at various offsets).
2. **RE `bin/VCDS.exe`** (authoritative). Tools available: `objdump -d` (LLVM, can
   disassemble i386 PE sections), `strings`, `nm`; `python3` (you MAY
   `pip install capstone pefile` — attempt it; if no network, fall back to objdump).
   - `strings` for `.clb`/`.rod`/`[CMP]`/`Labels`/`UDS_EV` references and any crypto
     constants (look for the observed header bytes `e6 42 7c 2e 26 10`, or standard
     constants: AES S-box, TEA/XTEA `0x9E3779B9`, RC4 KSA loops, CRC/zlib tables).
   - Locate the function that opens `*.clb`/`*.rod` and follow its buffer transform.
3. **Produce a decoder** (Python is fine for the PoC; a Rust port can follow) and
   **validate**: a decoded `.clb`/`.rod` CMP section must yield readable measurement
   names/units comparable to the plaintext `.lbl` style
   (e.g. "Engine Speed", "RPM", "Coolant Temperature", ranges).

## Deliverable
Write findings + any working decoder to this `research/clb-crack/` directory:
- `FINDINGS.md` — what the format is, the algorithm/key, how confident, what's left.
- `decoder.py` (or note why not achievable) + a couple of decoded sample outputs.
Do NOT modify the `crates/` production code. This is research only.
Report honestly if it turns out to need tooling/time beyond reach — a precise
"here's exactly where the decode routine is and what it does" is itself valuable.
