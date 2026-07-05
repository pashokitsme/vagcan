# Dynamic attack playbook — recover `K_epoch` from live old-VCDS memory

Owner runs the old x86 VCDS + cable in the ARM Win11 VM; I analyze the artifacts on
macOS. VMProtect hides code, not the **live AES round keys** in memory (used every
frame to XOR the b8/b7 link cipher). We dump memory mid-session and scan for the AES
key schedule — recovering `K_epoch` without touching a debugger (low anti-debug
friction). One session yields BOTH: (a) offline decode of that session incl. the VIN,
(b) `(K, b6-nonce, counter)` tuples across ~40 epochs to reverse the app-side KDF for an
extensible own-tool driver.

## USB capture NOT needed (USBPcap is an x86/x64 kernel driver — won't load on ARM64
Windows). The **memory dump alone** carries what we need: the derived `K` AND the KDF
inputs (b6-nonce in a TX buffer, the session counter, the static secret table, and often
recent b8/b7 blocks in RX/TX buffers). One dump → recover `K`, locate the inputs, and
hypothesis-test the KDF (`K = KDF(static_table, counter, b6-nonce)`) — one (inputs→K)
pair confirms/refutes a formula. If we later find we do need live wire bytes, the fallback
is Windows ETW USB tracing (`wpr`/`logman`, native to ARM64), not USBPcap.

## What the owner does in the VM (one session)

1. **Tool (install once):** **Process Hacker** (ARM-native; dumps the emulated x86
   process). Alt: Task Manager → right-click VCDS → *Create dump file* (full user dump).
2. **Run old VCDS**, connect to the car (ignition on), and **do a full Auto-Scan** (or at
   least open the engine + read vehicle info / VIN, DID F190).
3. **While VCDS is actively reading** (link keyed, mid-scan), dump `VCDS.exe`:
   - Process Hacker → right-click `VCDS.exe` → **Create dump file** (full). If that's huge
     (the image has a ~4 GB virtual VM section but committed RAM is far less), prefer
     Properties → **Memory** tab → save the **RW Private** committed regions (that's where
     the cipher context + buffers live) — smaller and sufficient.
4. **Zip the dump** and send it. Take **two dumps a few seconds apart** if easy (lets me
   see which state advanced — pins the counter). Note the car's real **VIN** if known
   (instant decode confirmation).

Repeat cheaply: since it's just a memory dump (no capture, no debugger), a few dumps from
different scan points give multiple epochs' `(K, inputs)` — more KDF-reversal signal.

## What I do (macOS, offline, from the dump alone)

1. `research/clb-crack/aes_ks_scan.py <dump>` → recover every AES-256 `K` in the dump
   (verifiable, no false positives; handles the LibTomCrypt word-swapped layout).
2. **Validate `K` against the EXISTING captures' known keystreams** where the epoch
   matches (`KS_cid == AES256(K).enc(IV_TABLE[cid])`), and against any b8/b7 blocks found
   in the dump's own buffers → decode them to UDS (VIN if F190 was read).
3. **Locate the KDF inputs in the dump:** the static secret table (match/adjacent to the
   arm64 table @0x140171730 — search the dump for it or for high-entropy 128-byte
   constants), the session counter, and the last b6-nonce (in a TX buffer). Build
   `(K, static_table, counter, b6-nonce)`.
4. **Reverse the app-side KDF by hypothesis-testing** on that tuple (SHA256/AES compositions
   of table‖counter‖nonce, truncations, HMAC forms). One confirmed formula → our own tool
   computes `K` for its own `b6` → opens any ECU with our own UDS → extensible driver (no
   VCDS, no replay). Multiple dumps give more tuples if one isn't enough.

## Why this beats the alternatives
- No devirtualization (VMProtect wall avoided): we read runtime **values**, not code.
- No debugger fight: an external memory dump is far less anti-debug-sensitive than
  breakpoints under x86 emulation.
- One session delivers the VIN now AND the data to reverse the KDF for the extensible
  path. If the KDF turns out to fold in a secret we can't reproduce, we still have (a)
  the decode capability and can fall back to generic-CAN for the product.

## Fallback if memory-dump is blocked
If VMProtect scrubs/relocates the key or the dump can't be taken cleanly: escalate to
x64dbg + ScyllaHide (native x86 Windows preferred) to breakpoint AES-setkey, or pivot to
the generic USB-CAN path (`vag-can`) for the extensible product.
