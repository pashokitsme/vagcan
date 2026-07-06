# Recovering the `.rod` per-record `product` term from VCDS process dumps

**Question.** The `.rod`/`TTTEXT.ROD` decoder decrypts every record except the
FIRST cipher block, which needs an 8-byte IV whose bytes `[3:8]` carry a
per-record `product` term (computed at runtime, not stored in the file). This
blocks the 7.6 MB `TTTEXT.ROD [TXT]` name table and 2 of 3 `MWB` measurement
sections offline (see `research/rod-measurement-feasibility.md`). Can we recover
the `product` term — or the decoded names it gates — from the five existing
VCDS-RUS process minidumps?

**OUTCOME: GOOD.** We could NOT recover the `product` derivation itself (BEST),
because the raw `TTTEXT` ciphertext was not resident in any dump (VCDS was not
mid-`TTTEXT`-decrypt at capture time). **But we recovered the thing the
`product` term gates — the decoded human names — directly from the heap: 11,479
unique VAG label/measurement names resident across the five dumps** (8,313
unique in `VCDS.exe.dmp` alone). No cipher, no product term needed for these.

Confidence: HIGH for the recovered names (lifted verbatim, CP1251, validated
against known measurement vocabulary). The `product`-formula gap is unchanged
from the feasibility doc; §5 gives the exact one-shot dump recipe to close it.

---

## 1. What we needed to recover (the IV / `product` construction)

From `crates/vag-data/src/rod.rs::rod_block0_iv` and
`research/clb-crack/decrypt_modern.py::rod_block0_iv` (identical logic):

```
seed = tag[0:3]  ||  product_bytes[0:5]                 (8 bytes)
s[i]  = (seed[i] + KS[(m*(i+2)) & 0xff]) & 0xff          m = tag[1]
IV[i] = (s[i] * MT[OFF_ROD[i]]) & 0xff
        OFF_ROD = [0x07,0xca,0x22,0x99,0x3e,0x88,0xc3,0x76]
product_bytes = (product & (2^40 - 1)).to_bytes(5, "little")
product       = ∏ chars of a runtime "record-string"    (64-bit)
```

Key structural facts:

- `IV[0:3]` comes only from the section tag → **always exact** (confirmed: for
  `TTTEXT.ROD [TXT]`, block0 decrypts to `78 da 54 …`, the intact zlib magic).
- `IV[3:8]` comes only from `product_bytes[0:5]`, and each byte is an
  **independent** per-byte transform of one product byte. So the unknown is
  exactly the 5-byte `product_bytes` (equivalently the scalar `product` mod
  2^40), and it is **per-section** (one `[TXT]` section ⇒ one `product`).
- The `record-string` is a runtime buffer (`x22`, from `singleton+0x18`,
  truncated at `'.'`), NOT any file field — so `product` cannot be computed
  offline (confirmed earlier: every file-byte candidate yields `product==0`).

**Why blind brute force is out.** For the zlib `[TXT]` section, only block0's 8
plaintext bytes depend on the IV; bytes 0–2 are already correct (tag), bytes
3–7 (5 bytes) are unknown. Those 5 bytes all fall inside the DEFLATE
dynamic-Huffman header's code-length-code region, which zlib only validates
*after* reading all of them — there is no per-byte early-reject oracle, so it is
a full 2^40 search with a full-inflate test each. Infeasible. Recovery must come
from memory, not computation.

---

## 2. Hunt A — is the raw `TTTEXT` ciphertext / IV / product resident? → NO

Searched all five dumps (`VCDS.exe.dmp`, `VCDS-2..4`, `VCDS-after-scan`) for:

| probe | result |
|---|---|
| `TTTEXT.ROD [TXT]` block0 ciphertext `43 e5 d4 da 14 e4 c3 54` | **absent in all 5** |
| tag-IV prefix `38 bc 58` (product-independent IV[0:3] for `[TXT]`) | 1 hit/dump, unrelated (a code immediate, not adjacent to cipher) |

The `[TXT]` cipher block never appears, so VCDS was not decrypting `TTTEXT.ROD`
at capture time. Therefore the **materialised 8-byte IV and the `product` term
are not recoverable from these dumps** (the IV lives in `x19[0:8]` only during
the decrypt call). This rules out the BEST outcome from the existing dumps.

The resident decoded `MWB` `id,code` rows *are* present (e.g. `043439,4.
043900,_5 095490,23 …` at ~332 MB in `VCDS-3`), but they match the
already-decodable `EV_…VW48` file (`product==0`) — no nonzero-`product` section
was resident to back-solve either.

---

## 3. Hunt B — the DECODED name table IS resident → GOOD

Adjacent to a resident `TTTEXT-RUS\0` filename string (offset ~332 MB in
`VCDS-3.exe.dmp`) sits an array of decoded name records. The bytes right after
the filename decode as CP1251 `Температура` ("Temperature") — i.e. the runtime
name-resolution output, the very data the `product` term gates, sitting in
cleartext in the heap.

**Record layout** (VCDS-RUS x86; fixed 72-byte stride), reverse-engineered from
the hex around the first entries:

```
record+40 : constant tag dword   a4 92 57 00
record+44 : u32  name length
record+56 : inline CP1251 name bytes (NUL-padded)
```

Scanning every occurrence of the tag dword and reading `(len, name)` yields the
whole resident set. Reusable tool: **`research/clb-crack/rod_name_harvest.py`**.

**Harvest (records / unique names):**

| dump | records | unique names |
|---|---|---|
| `VCDS.exe.dmp` | 25,302 | 8,313 |
| `VCDS-2.exe.dmp` | 10,411 | 4,260 |
| `VCDS-3.exe.dmp` | 9,804 | 3,941 |
| `VCDS-4.exe.dmp` | 9,298 | 3,802 |
| `VCDS-after-scan.exe.dmp` | 9,427 | 3,690 |
| **union** | — | **11,479** |

---

## 4. Validation — concrete names lifted from previously-blocked content

Verbatim from `VCDS-3.exe.dmp` right after `TTTEXT-RUS\0` (CP1251, glossed):

| resident bytes → CP1251 | meaning |
|---|---|
| `Температура` | Temperature |
| `Воздушный зазор` | Air gap |
| `Состояние` | Status / State |
| `Объект 1: ID` / `Объект 2: ID` | Object 1/2: ID |
| `Код клавиши 1` / `Код клавиши 2` | Key code 1/2 |
| `Код события 1` / `Код события 2` | Event code 1/2 |
| `По умолчанию` | Default |
| `НЕИЗВЕСТНЫЙ` | Unknown |

ASCII entries in the same region: `Coded_Value`, `alert_code`, `audio_bitrate`,
`not_available`, `EV_GatewNF_013`, `EV_GATEWNF`, `V03935243KR`, `3Q0-907-530-B`.

These are unambiguously VAG measurement/label names — exactly the data that is
crypto-blocked offline (`TTTEXT.ROD [TXT]`, `product != 0`). We obtained them
**without** the cipher or the `product` term.

**Scope / honesty caveats.**
- This is the **Russian** localisation (`TTTEXT-RUS`), and the resident set is
  whatever the live session had already resolved (~8k unique in the fullest
  dump). It is a large chunk, but not provably the entire name table.
- The records store the resolved **name string** only; the 6-digit `MWB` index
  → name **linkage is NOT present as an integer** in these structs (the join was
  already performed by index). So this recovers *names*, not a rebuildable
  `id → name` map. Building an offline `id → name` table still wants either the
  raw `TTTEXT` decrypt (needs `product`, §5) or a dump of the index array too.
- This does not touch the *other* two feasibility blockers (the UDS DID and the
  COMPU-METHOD scaling), which are absent from all readable `.rod` payloads
  regardless — see `research/rod-measurement-feasibility.md §2–3`.

No change to any crate is proposed: `decode_rod` remains offline-blocked for
`product != 0`; the names are a dump-only artifact, harvested by the standalone
script.

---

## 5. One-shot dump recipe to reach BEST (recover the `product`) 

The existing dumps miss it only because none was captured during a
`TTTEXT.ROD` decrypt. To close `IV[3:8]` for good, the owner captures **one**
dump while VCDS opens `TTTEXT.ROD` (or `TTTEXT-RUS.ROD`):

- Target: `VCDS-arm64-unpacked.exe`, ImageBase `0x140000000`, raw-`.rod`
  decrypt fn at `+0x33900`; IV build spans `+0x33814 .. +0x33b94`.
- **Break at `ImageBase+0x33b94`** (just before the shared TEA-CBC core call)
  and **dump the 8 bytes at `[x19]`** = the materialised IV for the section
  about to be decrypted. For the `[TXT]` section that single 8-byte value *is*
  the answer: `IV[0:3]` will read `38 bc 58` (sanity check), and `IV[3:8]` are
  the bytes we need. Invert the per-byte transform (`s = IV*MT⁻¹`, then
  `product_byte = s - KS`; the multiply is invertible when `MT[OFF_ROD[i]]` is
  odd) to get `product_bytes`, or simply feed the 8-byte IV straight into a
  decrypt.
- Equivalent alternative: **break at `ImageBase+0x33910` and dump `x22`** (the
  ~0x80-byte record-string); `product = (∏ its chars) & (2^40−1)`.
- Because `product` is per-section, one capture per distinct section-string
  closes that file. The big `[TXT]` section is a single capture.

With that value, `rod.rs::rod_block0_iv` gains a `product` argument for the
`[TXT]` record and the entire name table inflates offline — the BEST outcome,
deferred to a single targeted grab.

---

## Reproduction

- `research/clb-crack/rod_name_harvest.py <dump.dmp>` — harvests resident names
  (used for §3/§4). Dumps are gitignored PII and are NOT committed.
- Cipher/IV facts cross-checked with `research/clb-crack/decrypt_modern.py`
  (`rod_block0_iv`, `rod_section_cipher`) and `crates/vag-data/src/rod.rs`
  (unchanged).
