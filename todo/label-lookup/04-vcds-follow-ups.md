# label-lookup/04 — what is left after the VCDS registry and the units record

`label-lookup/02` (scalings from a VCDS installation through its `RM.rod` registry, PR #15) and
`label-lookup/03` (the car's units recorded by the live commands, `dev survey` removed, PR #18)
were merged on 2026-09-28 and moved to
[`.archive/tasks/done/label-lookup/`](../../.archive/tasks/done/label-lookup/). This file carries
what they left open, each with where it was found. Nothing here is started.

**2026-10-21:** item 4 is done (PR #39, 2026-10-20: a unit is looked up in VCDS's own order,
version and project before the bare name, the project from `chassis.clb` — the gateway reads
`EV_GatewNF_SK37 → EV_GatewNF_VW37` now). Item 8 no longer exists: since PR #56 a VCDS run reads
VCDS alone, and `registry::ensure` reads a unit whether or not an ODIS project describes it. Since
PR #50 the pool is one per machine, in `~/.vagcan/vcds/`; items 5 and 7 are read against that.

## For the owner to decide

1. **The gateway's four identification reads.** Since #18 the gateway (`0x710`) is asked
   `F187`/`F197`/`F19E`/`F1A2` on every run of `watch`, `measure` and `units --identify`, as the
   survey always asked it; without them its channels are never read. The controller added them
   on 2026-09-28 and told the owner. To undo: drop the gateway from `units::to_identify`'s walk
   (03, "Must not").
2. **A plain `[MWB]` list's first row** (02, "Left" 7). A nonzero `product` can spoil the row
   number's last three digits, so the first row of a plain list is dropped. A check that keeps
   29 of 29 such rows in 26.3 exists (a list's two-glyph code is a function of the registry row);
   with it, a wrong row drops from about one in 1,300 to about one in 5,000 — not zero.
3. **A witness for the one sweep left** (03, "Left"). `units --identify <unit>` runs with
   `witness: None`; `AsyncUdsClient::read_data_by_identifiers` and
   `anomaly::Monitor::seed`/`heard`/`silent_span` have no caller outside tests. Reading a witness
   is a new request. Until it is decided they stay, as written to be called.

## Open, without the car

4. ~~**Which platform file a unit reads**~~ — done 2026-10-20, PR #39 (above). (02, "Left" 1). The first readable in name order wins,
   so the gateway reads `EV_GatewNF_AU37`, not `_SK37`; the brands' lists differ by 1–5 rows of
   the same identifiers. Choose by the car: `chassis.clb`, as VCDS does.
5. **A Russian-only installation** (02, decision 3 and "Left" 3). Since PR #50 (2026-10-21) a
   machine keeps one installation, so the English one the decision names would be a second
   installation kept beside it — the owner's call before any of it is built. Its text and unit tables are
   shifted, so its channels are named by their identifier in hex and carry no ODX id, and a
   `dash.toml` `unit:IDE…` reference cannot resolve there. The owner's decision: take names and
   units from an English installation on the same machine, by text id and unit id — first check
   those ids agree between the two installs.
6. **Keys are cached by file name** (02, "Left" 6). A key from another build of the same name is
   tried, refused and searched again; caching by the section's own bytes would end that.
7. **The shared pool can mix two builds** of one language (02, "Left" 5): the copy is
   freshness-gated per file. Step 5 reads the installation itself; the fault chain does not.
8. ~~**VCDS's fill for units ODIS describes, on a car recorded after a pair setup**~~ — gone with
   the fill, PR #56, 2026-10-21 (above). (03, "Left").
   `registry::ensure` skips ODIS-described units, to spare minutes of key search, so such a car
   gets that fill only from the next `setup`.
9. **Two `setup` runs at once share the pool** (review, 2026-09-28): step 1's sweep of leftover
   `.<name>.<pid>.copying` files would remove the other run's copy in flight. Skip a file whose
   pid is still running.
10. **Not built** (02, "Left" 4): the `dev vcds` command that prints one unit's rows; `[INC]` for
    the fault chain (`UnitLookup::NoSection`).

12. **`rod::recover_classic_key` has no caller since PR #40** (cleanup pass, 2026-10-21;
    `research/tools/orphans.py` between `0ad5502` and `f58678c`). It searched one classic
    section's key for a registry read; #40 opens every classic section with the key its file name
    gives and dropped that search from `registry::read_section`. Uncalled, and its reason gone with
    the closed-form key — unless a classic section ever fails to open under its own name, which
    nothing has shown. The owner's call: delete it with its test, or keep it for that case.

## Open, with the car

11. **`22D2` on the cluster** (02, decision 2): nine bits in VCDS, read as sixteen by the proven
    row. A drive through the speed range where the ninth bit flips settles it.
