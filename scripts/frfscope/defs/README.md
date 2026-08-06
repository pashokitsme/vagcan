# Definitions

TunerPro XDF definitions used to name maps in a Simos18 calibration. All are
community definitions from [`joeFischetti/SimosDefinitions`](https://github.com/joeFischetti/SimosDefinitions)
— the only public Simos18 corpus. **None targets our software version**; see
`../../../research/stage1-frf-pipeline.md`.

frfscope picks the **alphabetically first** `*.xdf` here when `--xdf` is omitted,
so the preferred default must sort first. Today that is `SC8S30_…` — deliberate,
per the ranking below.

| File | Calibration | Engine | Tables | Gate over our `SC800O10` CAL |
|---|---|---|---|---|
| `SC8S30_8v0906264L.xdf` | `SC800S30` | **1.8 TFSI** | 192 | **139 ok / 47 flat / 6 noisy (72 %)** ← default |
| `SC8S50_8V0906259K.xdf` | `SC800S50` | 2.0 TFSI | 143 | 93 ok / 41 flat / 9 noisy (65 %) |

S30 wins on every axis that matters: it is the 1.8 l definition (our engine), has
more tables, and decodes more of our binary into structured values. Concretely,
it names boost control — wastegate setpoint (−1.0…1.0 normalised), Boost
Integrator and Boost P/D (40…167) — which S50 does not carry at all.

## Why neither is correct, and what would be

`data/box_codes.csv` in VW_Flash gives each software version's ECM3 monitoring
region as a CAL-relative offset — a free layout fingerprint:

| Calibration | ECM3 start | Distance from ours |
|---|---|---|
| **`SC800O10` (ours)** | 55112 | — |
| `SC800O20` | 55112 | **0** — nearest neighbour, no public definition |
| `SC800O30` | 55328 | +216 |
| `SC800O40` | 55360 | +248 |
| `SC800S30` / `SC800S50` | 55724 | +612 |
| `SC800H85` / `SC800H64` | 53336 / 52484 | −1776 / −2628 |

Both bundled definitions sit +612 away, on a different ASW branch (`SC8I0…` vs
our `SC8E0…`). That is the measured reason torque-structure maps decode to
nonsense (~900–1350 N·m on a 1.8 l). A definition for `SC800O20`, `O30` or `O40`
would be far closer; none is published — commercial damos packs exist for O20,
O30 and O40.

Treat these as **name/structure/scaling templates**, never as address truth.
