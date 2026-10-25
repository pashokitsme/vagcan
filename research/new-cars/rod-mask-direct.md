# The shifted-`.rod` mask over a genuine interface — the direct path, scoped

**2026-10-25.** Static RE of `.archive/research/clb-crack/bin/VCDS-arm64-unpacked.exe`
(PE ARM64, ImageBase `0x140000000`, sha256 `db94f060…07eab57`), provider Ghidra 12.1.4,
read-only, no crack run, no code produced. Companion to `labels.md` §1.3 (which this note
re-scopes) and `.archive/research/labels/tttext2.md` §3.3b (the mask mechanism). All
confidence below is *static observation of a shipped artifact*; nothing was run against a
cable or a car.

---

## 0. Verdict up front

| question | answer | confidence |
|---|---|---|
| Is the per-file mask `D` derivable **offline**? | **No** — it is the interface's keyed reply to a `0x45` challenge (`tttext2.md` §3.3b). Unchanged. | certain |
| Was the "direct path" (vagcan asks the genuine cable the `0x45` challenges) blocked by an **unreversed custom re-key**? | **No — that pessimism was about the wrong binary.** In 26.3 arm64 the session-key install is **RSA key-transport**, standard crypto, with the private key embedded in the binary. | high (static) |
| Is there a separate "old scheme" key-install branch in this build? | **No.** `FUN_14007b140` (the sole AES-256 set-key) has **one** caller; every encrypted link in this build is keyed through the one RSA path. | high (single-caller) |
| Can `K` therefore be computed, not just captured? | **Yes, in principle** — `K = unpad(RSA1024_decrypt(embedded_privkey, wire_block))`. The block arrives **plaintext** before the cipher turns on, so it is capturable. | high (static) |
| What is genuinely unknown? | which init frame carries the wrapped-key block for **this** cable, and whether this cable negotiates the RSA link at all (vs. staying plaintext). Only a live capture settles it. | — |

This note does **not** overturn `labels.md` §1.3's project answer (ODIS for shifted units).
It overturns only one sentence of §1.3(b): "the direct path is blocked by an unreversed
cipher." It is not. See §5 for why both can be true.

---

## 1. The mask, and where it lives at runtime

From `tttext2.md` §3.3b, confirmed against this binary:

- A `.rod` is **shifted** when its `[CMP]` first field is 8 digits. Every section after `[CMP]`
  carries an 8-byte XOR constant `D` on its first-block IV; `D` is one value per file.
- On opening a shifted file VCDS builds a 32-byte challenge from the gate number `G`
  (`[CMP]`'s digits) and the file name, sends it to the interface as CB command **`0x45`**,
  and takes `D` = **the last 8 reply bytes, reversed**.

Addresses (absolute VMAs; runtime offset = VMA − `0x140000000`):

| what | VMA | note |
|---|---|---|
| `0x45` challenge/reply routine | `0x140073df8` | builds challenge, sends `0x45`, stores `D` |
| gate number `G` | `0x15148f6e0` | `DAT_15148f6e0`; `G > 999999` ⇒ shifted ⇒ call made |
| **mask `D`** (8 bytes) | **`0x15148f724`** | runtime offset `0x1148f724`; cleartext after each open |
| challenge table `T` (`rod_ks.bin`) | `0x140171730` | `c[i] = (G >> (T[1+i] >> 3)) & 0xff`, zero byte replaced via name hash |

The clone-poison from §3.3b is here too: interface type `DAT_1405e8108 == 0x1236` flips bit 7
of `D[3]` (`DAT_15148f727`) on some calls — a deliberate corruption of a known clone, not of a
genuine Ross-Tech interface.

**Route (c), unchanged and crypto-free:** read 8 bytes at runtime offset `0x1148f724` from a
live VCDS after each file open. Costed in `labels.md` §1.3(c) as a recurring capture campaign.

---

## 2. `0x45` rides the encrypted link — so the direct path needs the session key

Sender `FUN_14006cef0` branches on the per-cable cipher-active flag `cable+0x5cd0`:

```
cable[0x5cd0] == 0  →  plaintext:  [0x04][len][payload][xor]
cable[0x5cd0] != 0  →  encrypted:  first byte | 0xf0, body through cipher FUN_140073160, xor
```

(The binary prints `"Sending CB packet "` vs `"Sending encrypted CB packet "` on these two
branches.) `0x45` is sent during label-file reading, i.e. with the session up — so in practice
the flag is set and `0x45` is **encrypted**. Hence the direct path needs the keystream, hence
the session key `K`.

Link cipher (as prior work established, unchanged): `KS[cid] = AES256_ECB_encrypt(K, IV_TABLE[cid])`,
`cid = (msg_type + 1) & 0xf`, IV table 16×16 at **`0x140171d30`**; wire bytes are plaintext XOR
the per-channel keystream. `FUN_140073160` applies it, `FUN_140073150` selects the channel.

---

## 3. The session-key install is one RSA path (the re-scoping)

Received CB blocks are dispatched by `FUN_14006d6c8`. A `0xf0`-marked block received **while the
cipher is still off** (`cable+0xcd0 == 0`) is a key-install block and goes to `FUN_140072ec0`;
once the cipher is on, an equivalent block goes to `FUN_140072fa0` (decrypt, not install). So the
wrapped key arrives **in the clear**, before encryption starts — capturable on the wire.

`FUN_140072ec0` is the whole install, and it is the only one:

```c
FUN_140072ec0(cable, block, len):
    r = FUN_14007ce68(block+3, len, out[296], &outlen=256, …,
                      cable+0x5da0,      // key-id / version selector
                      &tag,              // must come back == 1
                      cable+0x5cd8)      // crypto ctx, parsed from the embedded key
    if r == 0 && tag == 1:
        memcpy(cable+0x5da4, out, min(outlen,256))          // K
        ks = FUN_14007b210()
        FUN_14007b140(ks, &IV_TABLE/*0x140171d30*/, cable+0x5da4)  // AES-256 key schedule
        cable+0x5cd0 = 1                                     // cipher active
```

`FUN_14007b140` has **one** caller (this one), xref-verified — so there is no second,
"old-scheme" key install anywhere in the build. Whatever interface raises an encrypted link in
26.3 arm64 is keyed through `FUN_14007ce68`.

`FUN_14007ce68` is a thin RSA wrapper:

| callee | role |
|---|---|
| `FUN_14007ac10(ctx+0x38)` | modulus length; checked `== len` (RSA: ciphertext = modulus size) |
| `FUN_14007d010` | RSA private operation (modexp/CRT; bignum malloc `0x1401318a0`) |
| `FUN_14007c858` | CRT recombine (two half-exponentiations `FUN_14007c6f0` + Garner XOR) then unpad |

`FUN_14007c858`'s unpad: leading `00`, skip the zero run, expect a `01` separator, copy the
payload after it — a standard block-type unpad, to be matched exactly against a real block, not
a custom scheme.

### 3.1 The embedded private key is present and is the one prior work found

Bytes at **`0x140171a30`**:

```
30 82 02 5d  02 01 00  02 81 81 00  d3 2e 7b bc e9 bf 88 53  13 8d b1 34 e4 19 fe 52 …
└ SEQ 605 ┘  └ ver 0 ┘  └ INTEGER n (128 B, RSA-1024) ────────────────────────────────…
```

A DER `RSAPrivateKey`, RSA-1024, modulus beginning `d32e7bbc…` — byte-identical to the key
`extract_rsa_key.py` pulled before (`n = 0xd32e7bbce9bf8853…`, `e = 65537`, `p·q = n` verified
there). Context init `FUN_140073248` parses it into `cable+0x5cd8`.

---

## 4. What the direct path concretely needs

**In hand already** (binary + prior work):

1. Embedded RSA-1024 **private** key — DER at `0x140171a30`, 609 B; `extract_rsa_key.py` pulls it.
2. AES IV table — 16×16 at `0x140171d30`.
3. Link layer — reversed: `S`/`M` markers, XOR checksum, `0xf0` encrypted marker, keystream
   apply `FUN_140073160` / select `FUN_140073150` (`research/vag-hex-framing.md`, `crates/vag-hex`).
4. `0x45` challenge construction and `D` extraction (§1, `tttext2.md` §3.3b).

**To do**, in order:

1. **One live init capture** — genuine cable with VCDS 26.3 (or vagcan's own init) — to (a) find
   the plaintext `0xf0` wrapped-key frame, (b) confirm this cable uses the RSA link at all, and
   (c) pin the key-id selector `cable+0x5da0` and the `cid → IV-row` map. This is the only step
   that needs hardware, and the only real unknown.
2. Implement in vagcan: RSA-1024 decrypt + the §3 unpad → `K`; `AES256_ECB(K, IV_TABLE[cid])`
   keystreams; the encrypted send/recv branch of `FUN_14006cef0`.
3. Precompute each shifted file's `0x45` challenge (offline, from `G` + name); send over the
   cable; decrypt the reply; `D` = reversed last 8 bytes. Cache `{file name → D}` under
   `~/.vagcan` (per the CLAUDE.md data rule — derived keys, not label data). Then shifted files
   read offline like classic ones.

**Effort:** the RE is essentially complete (this note). Remaining is capture + standard crypto +
link glue — on the order of days **if** step 1 shows a clean RSA path. Risk is entirely step 1.

---

## 5. Why this does not reopen the archive's "cannot derive offline"

Two different claims, both true:

- `tttext2.md` §3.3b / `labels.md` §1.3(b): **`D` is not a function of the label data.** Still
  true — `D` is the interface's keyed reply, nothing in the `.rod` yields it without the
  hardware. Offline-from-files is dead.
- This note: **the link that carries the `0x45` request is not an unreversed wall.** The earlier
  "unreversed old-scheme re-key, VMProtect, weeks, likely dead end" was measured on the **old
  x86 24.7 VMProtect build**; it does not describe 26.3 arm64, which is unpacked and keyed by RSA
  key-transport with an embedded key.

So the direct path is "ask the genuine hardware, over a link we can now key" — not "break a
cipher." It still needs the cable; it is still one `0x45` per shifted file; it does not open the
corpus for someone without the hardware. ODIS remains the project's answer for shifted units
(`labels.md` §1.3(a)); this is the VCDS-only alternative for an owner who has the genuine
interface.

### Caveat — legal / policy

Using the binary's embedded Ross-Tech private key for local interop on one's own genuine cable
is one thing; **shipping that key in `vagcan` is another** — the same redistribution line the
project already draws around label data (CLAUDE.md "No car-specific data"). Keep any extracted
key and any `{file → D}` cache under `~/.vagcan`, out of the checkout, and decide the shipping
question before writing step 2.
