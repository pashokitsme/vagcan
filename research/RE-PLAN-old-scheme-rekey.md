# RE dossier — old-scheme per-`b6` re-key (the one blocker to a live VIN)

Complete, self-contained brief for reversing the last unknown: how the clone cable
re-keys its AES session per ECU so a diagnostic channel opens. Everything needed to
work this offline is here; cable-dependent steps are marked **[CABLE]**.

## The blocker (precise)

- Each diagnostic ECU rides its own **key epoch**. The capture has 40 `b6` events;
  each opens exactly one channel (b6#1→0x39 auth, #2→0x9e, #3→0x43, … #15→0xf3).
- A channel's keystream is `KS_cid = AES256(K_epoch).enc(IV_TABLE[cid])`, `K_epoch`
  fixed per epoch. We only have `K_epoch` **empirically** (recovered per-channel
  keystreams from known-plaintext), never the derivation.
- **Live, replaying the capture's 2nd `b6` does NOT re-key the cable** — it stays in
  epoch-1 (byte-identical 0x39/0x38 blocks; the epoch-2 0x9e poll gets zero response,
  no wedge). Determinism holds only for the FIRST (fresh-cable) `b6`.
- ∴ we cannot open any ECU beyond epoch-1 session control until we can make the cable
  re-key. That derivation is the target.

## Prime lead: the `0x09` keyed exchange = the re-key challenge/response

Evidence (from `reading-ecus.pcapng` / `init-only.pcapng`, both):
- Shape: OUT `09 <idx> <7 bytes>` (9 total), IN `09 <7 bytes>` (8 total).
- Occurs as a **triplet idx 05,02,03 once per ECU-open**, plus a lone idx 01 during
  bring-up (seq 2) and at seq 99, and idx 01 leading each later triplet's epoch.
- **IN byte[3] is constant within an epoch's triplet** and changes per epoch:
  reading-ecus 0x48 (seq72-76) → 0x71 (118-122) → 0x80 (586-590); init-only 0x5e → 0x61.
  → a per-epoch key-derived tag. Strongly implies `0x09` carries/ratchets `K_epoch`.
- No trivial OUT→IN transform (`OUT^IN` is noise) → keyed (cipher/MAC). The algorithm
  is app-side, in the OLD x86 VMProtect build (protected) — static RE is hard but the
  keyed structure may still yield to analysis + the live oracle.
- Full `0x09` OUT/IN pairs for both captures are dumped by the analysis in the session
  log; regenerate with the snippet in "Tooling" below.

### Working hypotheses to test
1. **`0x09` (not `b6`) is the re-key.** `b6` = anti-clone genuineness only; `K_epoch`
   is established/ratcheted by the `0x09` triplet. Test: after auth, drive the capture's
   `0x09` triplet (verbatim) and see if a new channel opens (vs. our `b6`-only replay
   which didn't). **[CABLE]**
2. **`0x09` IN is deterministic in `K_epoch` + OUT nonce.** If replaying the capture's
   exact `0x09` OUT reproduces the capture's IN live (like `b6`/bring-up determinism),
   the epoch is reproducible by replay. Test: replay capture `0x09` OUT, compare live IN
   to capture IN. **[CABLE]**
3. **The re-key needs the cable's response consumed/acted on.** Our replay may have not
   waited for / fed back a `0x09` IN the app must echo. Analyse capture ordering.

## What is fully known (no need to re-derive)

- Wire framing, opcodes, link cipher (`plain=cipher^KS`), IV_TABLE (16 rows), selector
  `cid=(msg_type+1)&0xf`, off14=plaintext counter, off15 trailer rule — see
  `vag-hex-framing.md`. Encode/decode + ISO-TP implemented in `crates/vag-hex/src/link.rs`.
- Auth-advance: `0x39` completion `b8` off14 = `observed & 0xF8` (live-proven).
- New build = RSA-OAEP key transport (embedded RSA-1024 priv key @0x140171a30) — NOT what
  this clone uses; do not chase it for this cable.
- `b3`/`b4` = CAN addressing, `ID = (16-bit b4) >> 5`; engine 0x7E8 (`b4 …fd00`),
  gateway 0x77A (`b4 …ef40`, always installed). VIN (DID F190) → engine/gateway.
- `0x0b` = encrypted 40-byte cable EEPROM, NOT a VIN cache.
- Cable hygiene: `FT_SetVIDPID(0x0403,0xFA24)` + FTDI init (8N1, 9600→19200→115200,
  DTR/RTS low, 1ms latency) + join-worker-on-drop (clean FT_Close). Stable across runs.

## Offline work to do NOW (no cable)

1. **Exhaustive `0x09` analysis:** every OUT/IN pair across both captures, per-epoch;
   map idx→role, find the per-epoch tag structure (IN byte[3] and others), test whether
   IN is a keyed function of OUT + a per-epoch key (try AES/DES/TEA with candidate keys,
   MAC structures). Correlate `0x09` triplets to the epoch's `K` (recovered keystream).
2. **Static RE of the old x86 build** (`bin/VCDS.exe`, `VCDSLoader.exe`, `RT-USB.dll`):
   despite VMProtect, look for the `0x09`/re-key handler, any unpacked regions, and
   RT-USB.dll (it's rebadged FTD2XX — may expose the cable-side command IDs). The arm64
   build's `0x09` handling (even though it's the new scheme) may share the wire format.
3. **Capture diff prep:** exact OUT/IN interleaving around b6#2 (seq 142–176) vs. what
   our live `--deep` replay sent, to list every deviation to fix.
4. **Pre-build a batched live experiment** (one command, tries all hypotheses, reports)
   so the cable session is single-shot.

## Cable needs — NOT constant

- Offline (above) is the bulk and needs no cable.
- **[CABLE] ONE focused batched session** to test hypotheses 1–3 (all in a single
  `handshake`-style command that tries: capture `0x09` triplet replay, compares live
  IN to capture IN, and attempts the new-channel open). Expect 1–3 such sessions total,
  not continuous access. Clean-close is fixed, so no power-cycles between runs; a real
  re-key experiment could still fault the clone once or twice (physical replug).

## Tooling (all in `research/clb-crack`, `.venv/bin/python`)
- `usbpcap.py` (`reassemble_frames`), `link_cipher.py` (IV_TABLE, keystream recovery),
  `disfn.py`/`xref.py` (AArch64), `extract_rsa_key.py`.
- Regenerate the `0x09` dump: reassemble frames, filter `payload[0]==0x09`, print
  `(seq, dir, payload.hex())`; group by the triplet boundaries.
