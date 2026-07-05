# Dynamic attack playbook — recover `K_epoch` from live old-VCDS memory

Owner runs the old x86 VCDS + cable in the ARM Win11 VM; I analyze the artifacts on
macOS. VMProtect hides code, not the **live AES round keys** in memory (used every
frame to XOR the b8/b7 link cipher). We dump memory mid-session and scan for the AES
key schedule — recovering `K_epoch` without touching a debugger (low anti-debug
friction). One session yields BOTH: (a) offline decode of that session incl. the VIN,
(b) `(K, b6-nonce, counter)` tuples across ~40 epochs to reverse the app-side KDF for an
extensible own-tool driver.

## What the owner does in the VM (one session)

1. **Tools (install once in the VM):**
   - **Process Hacker** (ARM-native; reads the emulated x86 process memory) — for the
     memory dump. Or `procdump -ma` (Sysinternals) if it runs under emulation.
   - **USBPcap** (already used for the existing captures) — to capture the session USB.
2. **Start USBPcap** on the cable's USB device (same as prior captures).
3. **Run old VCDS**, connect to the car (ignition on), and **do a full Auto-Scan** (or
   open the engine + read vehicle info / VIN). This makes VCDS open many ECU epochs and,
   crucially, **read the VIN (DID F190)** — which is NOT in the existing captures.
4. **While VCDS is actively reading** (mid-scan, link keyed), dump the VCDS.exe process
   memory:
   - Process Hacker → right-click `VCDS.exe` → **Create dump file** (full). If the full
     dump is huge (the image has a ~4 GB virtual VM section), prefer dumping only the
     **Private / committed heap regions** (Process Hacker → Properties → Memory → save
     the RW private regions), where the cipher context lives.
5. **Stop USBPcap.** Zip both artifacts.
6. **Send me:** the memory dump (zipped) + the `.pcapng`. Note the car's real VIN if you
   know it (lets me confirm the decode instantly).

## What I do (macOS, offline)

1. `research/clb-crack/aes_ks_scan.py <dump>` → recover every AES-256 `K` in the dump
   (verifiable, no false positives; handles the LibTomCrypt word-swapped layout).
2. Cross-check each `K` against the pcap's known keystreams:
   `KS_cid == AES256(K).encrypt(IV_TABLE[cid])` (link_cipher.IV_TABLE) → map each `K` to
   its epoch/channel, confirm the model.
3. **Decode the whole session** with the mapped K's → read the VIN + all scanned ECU data
   (immediate goal, proven on real data).
4. Build `(K, b6-nonce, session-counter)` tuples across the ~40 epochs; correlate to
   reverse the **app-side KDF** (`K = KDF(static_table, counter, b6/b7 nonce)`, symmetric
   AES/SHA per agent B). Extract the static table from the dump/binary. Once the KDF is
   reconstructed → our own tool computes `K` for its own `b6` → opens any ECU → extensible
   driver (no VCDS, no replay).

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
