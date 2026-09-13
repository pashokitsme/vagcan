# Addendum: an UNPACKED VCDS binary is now available

The first pass was blocked because `bin/VCDS.exe` (x86) is packed (VMProtect/Themida).
A second, DIFFERENT build was found and is NOT packed:

- `bin/VCDS-arm64-unpacked.exe` — PE32+ **AArch64** (native ARM64 Windows build),
  overall entropy **6.20** (normal), 2729+ readable strings, standard named sections
  (`.text` 0x1551bc @ VMA 0x140001000, `.rdata` @ 0x140157000, `.data` @ 0x1401ba000).
  `objdump` reads it as `coff-arm64` and disassembles it: `objdump -d` works.

This binary is the authoritative, tractable RE target for the modern `.clb`/`.rod`
cipher. It directly references the label files in cleartext strings:
`.\UDS_EV\`, `.rod`, `.\UDS_EV\STRUC.ROD`, `.\UDS_EV\Chassis.clb`, `Chassis.clb`,
`redir.rod`, `%sTTText-%s.ROD`. It also imports `CryptAcquireContextA` / `CryptGenRandom`
(Windows CryptoAPI — may or may not be used for label decryption; verify).

`bin/VCDSLoader.exe` (x86, entropy 7.55) is just a launcher/updater — NO `.clb`/`.rod`
or crypto strings — ignore it for the crack.

## What to do with the unpacked ARM64 binary
1. Disassemble with `objdump -d bin/VCDS-arm64-unpacked.exe` (AArch64). Optionally
   `pip install capstone` for scripted disassembly (Capstone supports ARM64).
2. Find the routine that opens `.clb` / `.rod` (xref the format strings above:
   `Chassis.clb`, `.rod`, `STRUC.ROD`). Its file-read → buffer-transform is the decoder.
3. Identify the 64-bit block cipher recovered structurally in FINDINGS.md (CBC, fixed key,
   fixed per-record IV, 8-byte blocks). Look for its round function / S-boxes / key
   schedule and, crucially, the **fixed key and IV constants** in `.rdata`/`.data`.
   - The block cipher is likely a classic 64-bit design (DES/3DES/Blowfish/XTEA/CAST/IDEA).
     Blowfish/DES have recognizable S-box/P-array constant tables in `.rdata` — scan for them.
   - Also check whether `CryptAcquireContext`/`CryptDecrypt` (CryptoAPI) is used with a
     hardcoded key blob instead of a custom cipher.
4. Recover key + IV + exact algorithm; wire into `decoder.py::block_decrypt` (CBC, IV reset
   per record). Validate: decoded `.clb`/`.rod` CMP/MWB records must yield readable
   measurement text (names, units, `Range:` clauses) like the plaintext `.lbl` style.

Container parsing + block extraction is already done in `decoder.py`; you only need the
cipher + key/IV. See `FINDINGS.md` for the full structural analysis and the SVCdec lead
(legacy key-table offset `0x26C46`, period-16 line-position term `2*((idx+20)&0xF)`).
