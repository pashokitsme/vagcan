# frfscope

Open a Simos18 firmware's calibration maps as graphs in a browser. Single-file,
stdlib-only renderer; extraction reuses `bri3d/VW_Flash`.

**Read-only.** Talks to no car, writes no ECU. See
[`../stage1-frf-pipeline.md`](../stage1-frf-pipeline.md) for the safety boundary.

## Usage

```bash
# zero-config: VW_Flash and a default definition are auto-discovered
python scripts/frfscope/frfscope.py FILE.frf

# override the definition; --report also lists the bad maps to stdout
python scripts/frfscope/frfscope.py FILE.frf --xdf DEF.xdf --report

# a raw calibration block needs no VW_Flash at all
python scripts/frfscope/frfscope.py FD_4.CAL.bin
```

Input type is resolved by extension: `.frf` (decrypt→odx→CAL), `.odx` (→CAL),
`.bin` (used as-is). Output is a self-contained HTML written next to the input
(or `--out PATH`) and opened in the browser (`--no-open` to skip).

### Discovery — nothing to wire up

- **VW_Flash** is found automatically: `--vwflash`, then `$VW_FLASH_DIR`, then
  `scripts/vendor/VW_Flash` (see `../vendor/README.md` to install it), then the
  cwd. If the running Python lacks `pycryptodome`, frfscope re-execs itself under
  VW_Flash's own `.venv` — so plain `python frfscope.py …` just works.
- **Definition**: with no `--xdf`, the first `*.xdf` under `scripts/frfscope/defs/`
  is used (currently an `SC8S50` near-match — see caveats).

In the page: 2D maps render as heatmaps, 1D as line charts. **Hover any cell for
its X · Y · value.** The filter box matches name, category, and the internal map
id. The color bar runs blue (low) → green → red (high).

### Value gate — which maps to trust

Each named map is judged from its own decoded grid: **ok** (structured),
**flat** (one value repeated — filler/unused/wrong address), or **noisy**
(cell-to-cell jumps ≈ garbage from a wrong address). The viewer defaults to
**values ok** so you look at real numbers; the segmented control switches to
flat / noisy / all, and bad cards carry a colored left border + the reason.
`--report` prints the same verdict to stdout with each bad map's address.

**This is a plausibility gate, not a correctness proof.** With a near-match
definition (e.g. an `SC8S50` XDF over an `SC8O10` binary), ~93/143 maps land on
structured data — but "structured" ≠ "certified correct". Only a definition
built for this exact software version (an O10 A2L) or hardware validation proves
a given map's address and scaling. The gate reliably catches *garbage*; it
cannot promise the survivors are exactly right.

## Dependencies

- `.bin` input: **none** (Python 3.10+ stdlib only).
- `.frf`/`.odx` input: the vendored `bri3d/VW_Flash` (`scripts/vendor/VW_Flash`,
  installed per `../vendor/README.md`) and its `pycryptodome`. frfscope re-execs
  into VW_Flash's `.venv` automatically, so you don't pick an interpreter.

## Caveats

- **The definition's addresses must match the binary's software version.** A
  near-match XDF (e.g. an `SC8S50` definition over an `SC8O10` binary) lands most
  maps on the wrong bytes — they show as flat/`0xFFFF`/noise. frfscope drops
  grids that run past the block; the rest you must sanity-check visually.
  Physical nonsense (a "torque" map reading 900 N·m, a lambda of 30) means the
  address is wrong for this binary, not that the tune is wild.
- Values use each axis's XDF `MATH` when it is a simple linear equation; anything
  else falls back to raw. Data is read little-endian, unsigned unless the XDF
  flags the field signed. Good enough to *see* a map; not a substitute for a
  correct definition when editing.
