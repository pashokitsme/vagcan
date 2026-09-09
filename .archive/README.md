# `.archive/` — retired paths kept as evidence

This directory is the project's morgue-with-a-purpose. Nothing here is live code or
current documentation; everything is a path the project **finished with** but chose to
keep as evidence. Three things land here: **research** whose findings are now
implemented and shipped (or that ended as a recorded dead end), **superseded specs**
that a later design replaced, and **finished task files** whose work is merged. The
governing rule is that things are **moved, never rewritten** — a `git mv` preserves the
document as it was written, so a claim keeps the date and the context that make it
evidence. If a fact here contradicts the live tree, the live tree wins; this is a record
of how the code got to be the way it is, not a description of how it is now. The
authoritative live map is [`../CLAUDE.md`](../CLAUDE.md) ("Project structure") and
[`../todo/README.md`](../todo/README.md).

Do not edit the substance of anything under here. The only maintenance this directory
takes is the kind that keeps it navigable: repairing a link a move broke, deleting build
junk (`.DS_Store`, `__pycache__/*.pyc`, `target/`, `.venv/`), and this README.

## `research/` — RE writeups whose results shipped, plus three dead-end notes

The three top-level `.md` notes are the **HEX-clone cable** story. That cable is dead
(crate and driver deleted); these are kept as negative results. The three
sub-directories were `git mv`d here from `research/` because their findings are
implemented in the live crates and no longer need to sit beside active work.

### Top-level notes (the dead HEX-clone cable)

| File | Question it answered | Verdict / where it lives now |
|------|----------------------|------------------------------|
| [`research/vag-hex-framing.md`](research/vag-hex-framing.md) | What is the clone cable's USB wire format? | Recovered in full: the plaintext `S/M` outer frame, the opcode vocabulary, and — later — the `b8`/`b7` diagnostic **link cipher** (a static position-dependent XOR keystream, *not* a block cipher). Authoritative as a negative result: the framing is known, but it only gets you to the ciphertext boundary. |
| [`research/clone-crypto.md`](research/clone-crypto.md) | Can the per-epoch AES session key `K_epoch` that encrypts the diagnostic channel be recovered? | **No** by any offline route — the key is derived app-side inside VMProtect-packed `VCDS.exe`; static RE, replay-shortcut, memory-dump and crack-DLL routes are all exhausted. The recommendation there is the route the project actually took: **bypass the clone with a generic USB-CAN dongle.** |
| [`research/vcds-rus-crack.md`](research/vcds-rus-crack.md) | What did the cracked VCDS-RUS build + its Auto-Scan give us? | The Auto-Scan ground truth (VIN, ECU part numbers) that became the **validation oracle** for `vagcan info`, plus the (since-deleted) minidumps that were the dynamic-attack material for `K_epoch`. |

### `research/labels/` — VW's label-file formats (findings shipped in `vag-data-labels`)

How Ross-Tech's compiled label/measurement corpus is decoded, and the fault-naming chain.
Everything here is implemented in [`../crates/data/vag-data-labels`](../crates/data/vag-data-labels)
and cached by [`../crates/data/vag-data-db`](../crates/data/vag-data-db).

| File | Question it answered |
|------|----------------------|
| [`research/labels/rod-labels.md`](research/labels/rod-labels.md) | The `.rod`/`.clb`/`.lbl` crack (TEA-CBC + zlib), and why measurement **scaling is live-only** (the STRUC refutation). Start here. |
| [`research/labels/tttext-codec.md`](research/labels/tttext-codec.md) | The `TTTEXT.ROD [TXT]` per-record substitution cipher → the recovered English name catalog (`names.json`). |
| [`research/labels/tttext2.md`](research/labels/tttext2.md) | `TTTEXT2.ROD` — does it hold a global measurement registry? **No: it refuses to open, blocked by a second IV regime.** A dead end. |
| [`research/labels/mux.md`](research/labels/mux.md) | `MUX.rod` — is *it* the registry? **No** — its grammar has no read identifier. A dead end. |
| [`research/labels/label-linkage.md`](research/labels/label-linkage.md) | Attacking the `.rod` corpus from the proven-measurement end to recover names and stored scaling. |
| [`research/labels/scaling-audit.md`](research/labels/scaling-audit.md) | Re-confirms the negative: `(DID → factor/offset/unit)` cannot be read out of the label corpus; scaling stays proven-on-car. |
| [`research/labels/codes-dat.md`](research/labels/codes-dat.md) | `Codes.dat`, the fault-text store — what it can and cannot name. |
| [`research/labels/fault-naming-hop.md`](research/labels/fault-naming-hop.md) | The last hop from a control unit's 24-bit fault number to words; powers `vagcan faults --labels`. |
| [`research/labels/odis-format.md`](research/labels/odis-format.md) | VW's ODIS-Service project on disk — three file formats; implemented in `crates/data/vag-data-labels/src/odis/`. |
| [`research/labels/odis-crib.md`](research/labels/odis-crib.md) | ODIS runtime data as a known-plaintext crib against the label ciphers. |
| [`research/labels/odis-project-mapping.md`](research/labels/odis-project-mapping.md) | VW's `S42` project-name → vehicle table (e.g. `SK37X` is a platform, not a car). Transcribed reference. |

Alongside the notes are the RE scripts that produced them: `odis_crib.py`, and the
sub-directories `rd-rod/` (the Python `.rod` reader — alphabet/codes/tables/solve/sweep),
`scaling-audit/` (`audit.py`, `glyphs.py`), and `tttext2-sweep/` (a Rust IV sweep plus
`rodread.py`/`textinspect.py`). These are RE tooling, not shipped code.

### `research/car/` — what the reference car answers

Findings about the one reference car (Škoda Octavia III, 1.8 TFSI, DQ200, VIN
`XW8AD4NE9JH008917`). Per [`../CLAUDE.md`](../CLAUDE.md), such facts are **evidence for a
decoder, not a table to ship**; the proven rows live under `~/.vagcan/`, outside the
checkout.

| File | Question it answered |
|------|----------------------|
| [`research/car/identifier-map.md`](research/car/identifier-map.md) | What each UDS `ReadDataByIdentifier` returns on the engine and gearbox, from two exhaustive sweeps plus driving recordings. |
| [`research/car/other-ecus.md`](research/car/other-ecus.md) | The rest of the car from two passive CAN captures. |
| [`research/car/whole-car-survey.md`](research/car/whole-car-survey.md) | Every control unit's answer to `vagcan survey` / `vagcan faults`, read live. |
| [`research/car/gearbox-state.md`](research/car/gearbox-state.md) | Which raw identifiers carry engaged gear and selector position on the DQ200. |

### `research/clb-crack/` — the `.clb`/`.rod` crack, RE scripts

The reverse-engineering workbench that cracked the modern (MQB) label cipher — **TEA
(CBC, two hardcoded keys, per-record IV)** — recovered from an unpacked AArch64 VCDS
build. The result is implemented in `vag-data-labels`. Read
[`research/clb-crack/FINDINGS.md`](research/clb-crack/FINDINGS.md) (status) and
[`research/clb-crack/INTEL.md`](research/clb-crack/INTEL.md) /
[`research/clb-crack/INTEL-2-unpacked.md`](research/clb-crack/INTEL-2-unpacked.md) (how
the unpacked binary broke it); [`research/clb-crack/NOTES-modern.txt`](research/clb-crack/NOTES-modern.txt)
is the algorithm writeup. The many `.py` scripts, `rod_crack/` (Rust), and the binary
fixtures (`crack_input.bin`, `rod_KS.bin`, `fixture_synthetic.*`) are the tooling and
its inputs. This directory carries an untracked `.venv/` and `rod_crack/target/` when
the scripts have been run; those are build junk, gitignored, not part of the archive.

## `specs/` — superseded designs

Both specs predate the pivot away from the HEX-clone cable and carry a `SUPERSEDED`
banner at the top pointing at what replaced them. The live design lives in
[`../CLAUDE.md`](../CLAUDE.md), [`../ARCHITECTURE.md`](../ARCHITECTURE.md) and
[`../USAGE.md`](../USAGE.md).

| File | What it designed | Why superseded |
|------|------------------|----------------|
| [`specs/2026-07-02-vagcan-cli-design.md`](specs/2026-07-02-vagcan-cli-design.md) | The original `vagcan` PRD, with `vag-hex` as the transport and `vag-cli`/`vag-core` crates. | Those crate names never shipped; the transport is now the generic slcan adapter. |
| [`specs/2026-07-03-vag-hex-transport.md`](specs/2026-07-03-vag-hex-transport.md) | The `vag-hex` cable-transport crate. | The HEX-clone path is dead (session KDF VMProtect-sealed); the crate is deleted. |

## `tasks/done/<subsystem>/` — finished task files

One subdirectory per subsystem, each holding the brief for a task that is
**done + reviewed + merged**. Many belong to the abandoned HEX-clone line
(`cable-actor`, `link-decode`, `research-keystream`, `session-key`, `usb-backend`,
`init-handshake`, `live-drive`) and describe work whose *code* was later deleted with
the cable — the brief stays as the record of what was attempted. Others
(`async-core`, `uds-async`, `generic-can`, `label-lookup`, `cli-app`, `dash`) fed the
live tree. New finished tasks retire here from `todo/` per the workflow in
[`../CLAUDE.md`](../CLAUDE.md).

| Subsystem | Task |
|-----------|------|
| [`async-core`](tasks/done/async-core/01-async-transport-trait.md) | async transport trait + mock |
| [`cable-actor`](tasks/done/cable-actor/01-cable-actor-handle.md) | `CableActor`/`CableHandle` mpsc/oneshot multiplex (HEX-clone; since removed) |
| [`cli-app`](tasks/done/cli-app/01-vagcan-doctor.md) | `vagcan` binary + `doctor` |
| [`dash`](tasks/done/dash/01-plan-format.md) | the dash plan format + its generator |
| [`generic-can`](tasks/done/generic-can/01-generic-can-backend.md) | generic CAN backend (the bypass that replaced the cable) |
| [`init-handshake`](tasks/done/init-handshake/01-plaintext-handshake.md) | plaintext open handshake (HEX-clone) |
| [`label-lookup`](tasks/done/label-lookup/01-fast-lookup.md) | fast label lookup (`vag-data`/`vag-db`) |
| [`link-decode`](tasks/done/link-decode/01-port-decode-rs.md) | port the link-cipher decode to Rust (HEX-clone) |
| [`live-drive`](tasks/done/live-drive/01-cable-live-drive.md) | HEX-clone talks live on macOS (since removed) |
| [`research-keystream`](tasks/done/research-keystream/01-reverse-16key-schedule.md) | reverse the 16-key link-cipher schedule (HEX-clone) |
| [`session-key`](tasks/done/session-key/01-trace-session-key.md) | classify the AES session-key derivation (HEX-clone) |
| [`uds-async`](tasks/done/uds-async/01-async-uds-client.md) | async UDS client + ISO-TP |
| [`usb-backend`](tasks/done/usb-backend/01-backend-trait-d2xx.md) | Backend trait + D2XX backend (HEX-clone) |

## Do not retry

These are recorded here precisely so no one spends the week again. Each is a conclusion
from the note cited, not a guess:

- **Do not try to recover the clone cable's AES session key `K_epoch` offline.** Every
  offline route (static RE, replay-shortcut, memory-dump, crack-DLL) is exhausted; the
  key is derived inside VMProtect-packed `VCDS.exe`
  ([`research/clone-crypto.md`](research/clone-crypto.md) §5). The project's answer is
  the generic USB-CAN dongle, not the clone.
- **Do not re-attempt the clone link as a block cipher.** The `b8`/`b7` diagnostic
  channel is a static position-dependent XOR keystream; that is already reversed
  ([`research/vag-hex-framing.md`](research/vag-hex-framing.md)). Knowing it still does
  not get you past the sealed session key above.
- **Do not look for a stored measurement `(DID → factor/offset/unit)` scaling in the
  label corpus.** It is not there; scaling is live-only. `TTTEXT2.ROD` and `MUX.rod`
  were the last candidates and both came up empty
  ([`research/labels/scaling-audit.md`](research/labels/scaling-audit.md),
  [`research/labels/tttext2.md`](research/labels/tttext2.md),
  [`research/labels/mux.md`](research/labels/mux.md)).
</content>
</invoke>
