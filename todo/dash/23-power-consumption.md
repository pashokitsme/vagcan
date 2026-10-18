# dash / 23 — electrical consumption in the status strip

**Subsystem:** dash · **Crates:** `vag-cli-core` (`dash.toml`, the plan), `vag-dash-render`
(the strip, the derivation), `vag-dash-fw` · **Needs the car:** yes, first

**State (2026-10-19):** recorded, not started. The status strip (the page's title and the
battery voltage across the top of each page) was built on 2026-10-19 without it: consumption
needs a derived channel, which nothing has yet, and currents nobody has read on the car.

## What the owner asked (2026-10-19)

The approved strip prototype (`Volt::StripLeft`) showed the consumption in watts beside the
voltage, `14.2V  480W`. It was left out of the strip's change and written down here.

## What it is

- **Consumption ≈ U × (I_alternator − I_battery).** The alternator feeds the car and charges
  the battery; what the battery takes is not consumed.
- **Engine off: ≈ −U × I_battery.** No alternator, the battery feeds everything.
- U is the strip's own `[strip] volts`.

## The gateway's channels (`0x710`), declared, not verified

| DID | text id | what | scaling as declared |
|---|---|---|---|
| `2A09` | `IDE01836` | battery current | A, ×0.001, s32 |
| `2A1C` | `IDE01855` | alternator current | A, ×0.1 |
| `2A0C` | `IDE01839` | battery state of charge | % |
| `122A` | — | `EM_electrical_system_current` | A, ×0.001 |

These are what the reference car's label data declares. They are evidence for a decoder, not a
table to ship (`CLAUDE.md`, "No car-specific data"): the owner names them in `dash.toml`, as
every other channel.

## Before any code

1. `vagcan watch --did 710:2A09,2A1C` parked, engine off, then running with loads switched on
   and off (lights, fan, rear window heater).
2. From that: the sign of the battery current (charging positive or negative), whether the
   factors are right, how often the gateway answers.
3. Whether `122A` is the consumption already, which would make the derivation unnecessary.

## What it needs

- **A derived channel.** Nothing derives a value on the board today: every channel is one read,
  scaled. A derivation of two or three channels (a product and a difference) has to be added
  to the plan, the generator and the board, with "no answer" from any input being no answer —
  never a zero. Its form in `dash.toml` is open.
- **In the strip:** left-aligned right after the voltage, a two-glyph gap between them, written
  `480W`; `--W` with no answer.
- **Per page like `volts`:** a page key turns it off; hidden on a page that shows the
  consumption channel as a cell.
- No `power` key exists until then: the strip's change added none (2026-10-19).
