# The shifted-`.rod` mask over a genuine interface — direct-path PoC

**2026-10-25; crypto corrections and PoC 2026-10-26.** Target:
`.archive/research/clb-crack/bin/VCDS-arm64-unpacked.exe` (PE ARM64, ImageBase
`0x140000000`, sha256 `db94f060…07eab57`), analysed read-only with REA / Ghidra 12.1.4.
Companion to `labels.md` §1.3 and `.archive/research/labels/tttext2.md` §3.3b.

No genuine interface or car was used. The evidence below is static observation of the shipped
binary, synthetic protocol tests and offline decoding of a real ROD. It is not a live-link claim.

---

## 0. Verdict up front

| question | answer | confidence |
|---|---|---|
| Is per-file mask `D` derivable from the ROD alone? | **No.** It is the interface's keyed reply to a `0x45` challenge. | certain |
| Is the direct path blocked by an unreversed custom re-key? | **No.** This build installs the session key through RSA-1024 **OAEP-SHA256**, and the encrypted CB body uses **AES-256-CBC**. | high (static) |
| Is there a second old-scheme key-install branch? | **No.** The sole AES-256 set-key routine `FUN_14007b140` has one caller, the RSA install path. | high (xref) |
| Is the PoC implemented? | **Yes.** The existing ROD parser accepts a mask directly; an example builds/decrypts CB blocks and unwraps a local RSA key. | measured |
| What remains unknown? | The genuine cable's init capture, outer USB framing and whether that cable negotiates this RSA link. The empty OAEP label also remains to be confirmed against a live vector. | — |

This does not change the project answer in `labels.md` §1.3: ODIS remains the practical route for
shifted units. It removes the earlier, incorrect claim that the direct route is blocked by an
unreversed cipher.

---

## 1. Mask and `0x45` challenge

From `tttext2.md` §3.3b, confirmed against this binary:

- An eight-digit first field in `[CMP]` is the shifted-file gate `G`. Every section except `[CMP]`
  carries one eight-byte XOR value `D` on its first-block IV.
- The file's uppercase stem (last path component, cut at the first dot) gives a name hash `h`:
  start with `h = stem[0]`, then for every character `h = (T[0] + (c ^ h)) & 0xff`, where `T` is
  the existing `rod_ks.bin` table.
- For `i = 0..31`, the challenge is:

  ```text
  c[i] = low8(G >> (T[1+i] >> 3))
  if c[i] == 0:
      c[i] = T[(i + (h >> 1 if i is even else h)) & 0xff]
  ```

  The ARM64 instruction is a 32-bit arithmetic shift. It equals a logical shift for the positive
  eight-digit gates accepted here.
- The challenge is CB command `0x45` with a 32-byte payload. The reply must have the same command
  and payload length. `D[j] = reply[31-j]` for `j = 0..7`: the last eight payload bytes, reversed.

Addresses (absolute VMAs; runtime offset = VMA − `0x140000000`):

| what | VMA | note |
|---|---|---|
| `0x45` challenge/reply routine | `0x140073df8` | all 32 challenge bytes, send, reply and mask extraction |
| gate `G` | `0x15148f6e0` | `G > 999999` selects the shifted path |
| mask `D` | `0x15148f724` | eight cleartext bytes after the reply |
| challenge table `T` | `0x140171730` | the bytes in `rod_ks.bin` |
| CB message builder | `0x14006b640` | header and payload chunks |

The clone-poison from `tttext2.md` §3.3b is also present: interface type
`DAT_1405e8108 == 0x1236` can flip bit 7 of `D[3]` (`DAT_15148f727`). The PoC does not reproduce
that clone-specific corruption.

The runtime-capture alternative is unchanged: read the eight bytes at runtime offset `0x1148f724`
after VCDS opens the file. That is the recurring capture campaign described in `labels.md` §1.3(c).

---

## 2. CB framing and AES-256-CBC

A plaintext single-chunk CB message is:

```text
[msg_type][00][command:be16][total_payload_len:be16][chunk_len:u8][payload]
```

The builder at `0x14006b640` permits at most `0xdb = 219` payload bytes in a chunk. A `0x45`
request or reply needs only one chunk.

Sender `FUN_14006cef0` branches on the cipher-active flag at `cable+0x5cd0`. With encryption on,
`FUN_140073160` produces an encrypted CB block:

```text
body       = zero_pad_to_16(plain_cb[2..])
cid        = (msg_type + 1) & 0x0f
ciphertext = AES-256-CBC-ENC(K, IV_TABLE[cid], body)
wire_cb    = [msg_type|0xf0][msg_type|0xf0] || ciphertext || xor_all_previous_bytes
```

The IV is reset for every call. This is CBC over the original CB bytes from offset 2, **not** the
`AES256_ECB(K, IV) XOR plaintext` keystream described in the first version of this note. Decryption
at `0x1400730d0` is ordinary CBC decryption.

| what | VMA |
|---|---|
| encrypted sender | `0x14006cef0` |
| channel selection | `0x140073150` |
| AES-CBC encrypt wrapper / implementation | `0x140073160` → `0x14007afd0` |
| AES-CBC decrypt wrapper / implementation | `0x1400730d0` → `0x14007aeb0` |
| 16 × 16-byte IV table | `0x140171d30` |

The PoC consumes and produces exactly these encrypted CB blocks, including their final XOR byte.
It deliberately does **not** add an outer `[04][len]…[xor]` transport frame: the decompiled outer
length/copy logic is ambiguous, and this project has no verified genuine-interface USB transport.

---

## 3. Session-key install: RSA-OAEP-SHA256

Received blocks are dispatched by `FUN_14006d6c8`. A `0xf0`-marked block received while the cipher
is off goes to `FUN_140072ec0`; once encryption is active, the equivalent block goes to the normal
decrypt path. Thus the wrapped key arrives before the session cipher turns on.

`FUN_140072ec0` passes `block+3` to `FUN_14007ce68`, then installs the recovered 32-byte `K` through
`FUN_14007b140` and sets `cable+0x5cd0`. The wrapped RSA block is 128 bytes. The PoC takes those
128 bytes separately, not a guessed outer packet containing them.

`FUN_14007c858` performs OAEP decoding: masked seed and masked DB, two MGF1 operations, `lHash`,
the zero run and the `01` separator. It is **not PKCS#1 v1.5**. The hash selector at `0x14007b270`
resolves `"sha256"`, so the PoC uses:

```text
K = RSA1024-OAEP-decrypt(private_key, wrapped_key)
OAEP hash = SHA-256
MGF1 hash = SHA-256
OAEP label = empty (pending confirmation against a genuine capture)
```

If a genuine exchange fails while RSA and AES known-answer tests pass, the OAEP label arguments
are the first interoperability detail to revisit.

### 3.1 Local private key, not a committed secret

The binary carries a 609-byte DER `RSAPrivateKey` at **`0x140171a30`**, loaded by `FUN_140073248`.
Its modulus begins `d32e7bbc…`; the existing extractor is
`.archive/research/clb-crack/extract_rsa_key.py` (not a `scripts/` subdirectory).

The PoC accepts a local PKCS#1 or PKCS#8 PEM and requires RSA-1024. It does not embed that key,
print the unwrapped session key, or write keys/masks into the checkout. Tests generate their own
synthetic RSA key.

---

## 4. Implemented PoC

The implementation is in the existing parser, not a parallel ROD implementation:

- `crates/data/vag-data-labels/src/rod/mod.rs` — `decode_rod_with_mask(data, file, mask)`, reusing
  the existing framing, name-derived IV, TEA-CBC and zlib paths. No key search or cache is needed.
- `crates/data/vag-data-labels/src/rod/shifted.rs` — eight-digit gate parsing, filename hash,
  challenge construction and reply-mask extraction.
- `crates/data/vag-data-labels/examples/rod-shifted.rs` — RSA/AES/CB offline driver. Crypto crates
  are dev-dependencies only; the product CLI is unchanged.

With `ROD` set to the file path:

```bash
cargo run -p vag-data-labels --example rod-shifted -- challenge "$ROD"
cargo run -p vag-data-labels --example rod-shifted -- request "$ROD" KEY.pem WRAPPED_KEY_HEX 3
cargo run -p vag-data-labels --example rod-shifted -- open "$ROD" --mask MASK_HEX
cargo run -p vag-data-labels --example rod-shifted -- open "$ROD" \
  --key KEY.pem --wrapped-key WRAPPED_KEY_HEX --reply ENCRYPTED_CB_HEX
```

`request` prints the gate, challenge, cid and encrypted CB request. `open --reply` validates the
checksum, lengths, command and zero padding before applying the recovered mask. Section status and
lengths go to stderr; decoded text goes to stdout. `MSG_TYPE` is restricted to `0..=15`, and the
reply parser intentionally supports only a single 32-byte `0x45` chunk.

---

## 5. Verification performed

Synthetic checks:

- known-answer 32-byte challenge, gate rejection and mask reversal in the parser tests;
- direct decoding of a synthetic shifted ROD by mask, with `[CMP]` unmasked and no cache/search;
- wrong-mask refusal for compressed sections;
- AES-256-CBC against NIST SP 800-38A (`f58c4c04…` first-block vector);
- synthetic RSA-OAEP-SHA256 roundtrip, tamper refusal, PKCS#1/PKCS#8 PEM parsing;
- encrypted CB roundtrip, emulated `0x45` reply, checksum and nonzero-padding refusal;
- `cargo clippy -p vag-data-labels --all-targets -- -D warnings` passes.

Real-file check without another brute-force run: `tttext2.md` §4.1 had already recovered
`iv[3..8] = b1 a4 81 5c a3` for `EV_HCP4Contr2OBDAU41X_BY64.rod [FFMUX]`, with anchor `0x4d`.
Combining that saved result with the name-derived IV gives the file's full mask locally. Neither
that mask nor the ROD was added to the repository.

Both `open --mask` and the complete `request` / emulated `open --reply` path were run against the
real local file. An independent Python/LibreSSL oracle generated the challenge, synthetic RSA-OAEP
input and AES-CBC reply; its challenge and encrypted request matched the Rust output byte for byte.
All sections opened at their declared lengths:

| section | decoded | bytes |
|---|---|---:|
| `CMP` | TEA | 54 |
| `ADP` | zlib | 968 |
| `INC` | TEA | 84 |
| `DTC` | zlib | 20,966 |
| `FFMUX` | zlib | 55,121 |
| `MWB` | zlib | 16,115 |
| `SLV` | TEA | 13 |
| `XPL` | TEA | 22 |

A one-byte change to the mask leaves every compressed section `Undecodable`, as intended.

These are strong offline interoperability checks, not evidence that a physical cable was contacted.

---

## 6. What remains for a live direct path

1. Capture a genuine interface's init exchange: locate the cleartext 128-byte wrapped-key block,
   confirm RSA negotiation and confirm the empty OAEP label.
2. Resolve the outer USB framing and observe real `msg_type` / cid values; only then connect the
   example's CB blocks to a transport.
3. Keep any extracted private key and future `{file, release → D}` cache under `~/.vagcan`, out of
   the checkout. The PoC currently has no mask-cache writer.
4. Treat a live capture as the acceptance test for transport integration. Do not infer it from the
   synthetic and offline checks above.

Offline derivation of `D` from label data is still impossible. This work supplies the standard
crypto and parser path for an owner who already has the genuine interface or a mask; it does not
open the shifted corpus without either. ODIS remains the project's actual answer for shifted units.

### Caveat — legal / policy

Using the binary's embedded Ross-Tech private key for local interop on one's own genuine cable is
one thing; shipping that key is another, under the same redistribution line as label data. Keep it
and derived masks outside the checkout.
