# dash / 18 — a channel's specified value, and drift from it

**Owner, 2026-09-14.** Several engine channels come in pairs: what the control unit asked for
and what it got. Show both, and shout when they part company for long enough to mean
something. Where a channel has no specified value, nothing changes.

Design agreed with the owner on 2026-09-14, in this order: the pair is written down by hand,
the screens show the **difference**, and the alarm needs the difference to *hold*.

## 1. What the car has

Proven on the reference car (`~/.vagcan/data/SK37X/measurements/8V0906264H.json`, the rows
`identifier-map.md` §2.1 established):

| what | DID | unit | scaling |
|---|---|---|---|
| Boost pressure, specified | `2029` (`IDE00190`) | bar | u16 be × 0.001 |
| Boost pressure, actual | `202A` (`IDE00191`) | bar | u16 be × 0.001 |

Declared by the ODIS project for this engine (`EV_ECM18TFS02*8V0906264*`), not yet read on the
car: `20BA` throttle actuator actual value (°) against `39AD` `Throttle_angle_commanded_value`
(°); `206D` `Throttle_valve_control_value` (%); camshaft phasing `2019`/`201A` and
`201D`/`201E`; fuel high pressure `293B` commanded; coolant `202C`/`202D` commanded.

**The pair is never guessed from names.** The three spellings above — "commanded value",
"actual value", "control value" — do not pair up by any rule that holds across units, and a
wrong pair would show a difference that means nothing. The owner writes the pair down.

## 2. The plan

`dash.toml` gains one optional key on a `[[channel]]`:

```toml
[[channel]]
ref = "01:IDE00191"       # boost, actual
setpoint = "01:IDE00190"  # boost, specified
label = "НАДДУВ"
decimals = 2
```

- The specified channel need not have a `[[channel]]` of its own: the builder adds it, at the
  actual's `hz`, and it is not offered as a cell.
- **Refused at build:** a `setpoint` whose unit differs from the channel's, one the resolved
  variant does not declare, one on another unit (see below), or a `setpoint` pointing at a
  channel that itself has one.
- `Channel` in `plan.rs` gains `setpoint: Option<u16>` — the plan index of the specified
  channel, resolved on the laptop like everything else the board never resolves.

**Same unit only, and that is what makes it free.** Both identifiers are due together on one
unit, so `Planner` asks for them in **one `22`** (a unit's due identifiers go in one request):
one exchange, and the two numbers come from the same moment. A pair across two units would be
two exchanges and two moments, and the difference between them would be partly the delay.

## 3. The screens

`Cell` gains `deviation: Option<f32>` — the firmware computes `actual − specified`; the
renderer only draws it. `None` where either side has not answered, drawn as a dash.

- **Values page (up to four cells):** label, number, **deviation**, unit. The deviation line is
  the small face, signed, without the unit (the unit is the line under it): `+0.07`.
  Four lines instead of three, so the row's numerals step down one face — 27 → 25 px tall on
  `bold_mono`. A cell without a setpoint keeps its three lines and the row keeps the larger
  face when no cell in it has one.
- **Chart page:** the deviation in the small face under the number. The header keeps the label
  and the scale's range and **drops the seconds** (owner: the seconds are not worth the room).
  A chart of a channel without a setpoint keeps the seconds.
- Nothing else moves: the link icons, the alarm inversion and the adapter screen are untouched.

## 4. The drift alarm

A new rule kind beside the existing threshold rules:

```toml
[[alarm]]
kind = "drift"
channels = ["01:IDE00191"]
page = "MAIN"
percent = 10          # fires when |actual − specified| exceeds 10 % of the specified value
release_percent = 6   # clears under 6 %
hold_ms = 1000        # and only once it has held that long
min_setpoint = 0.5    # below this specified value the rule says nothing
```

- **`hold_ms` is what makes it usable.** A turbocharger lags its own setpoint on every throttle
  stab; without a hold the rule fires on every gear change. The alarm trips only when the
  deviation has been past `percent` for that long without a break.
- **`min_setpoint` is the other half.** A percentage of a specified value near zero is noise —
  a throttle commanded to 0.2° would trip on 0.03°.
- Hysteresis (`release_percent`), the 2.5 s hold after the release and the button that silences
  the episode are the existing machinery (`vag_dash_render::alarm`), unchanged.
- The existing `direction`/`trip`/`release` rules stay as they are; `kind` defaults to the
  threshold rule so every `dash.toml` written so far still builds.

## 5. Built, 2026-09-15 (branch `setpoint-drift`)

Everything above is in the tree and hardware-free green: `Deviation` on a cell and the two
screens (`494df74`, `983e758`, `b546f6d`, `733aa43`), `setpoint` in `dash.toml` and the plan
(`83d6d35`), the drift rule in the alarm machine and the builder (`0fa3a07`), and the board
drawing the difference and reading a pair as a pair (`5816c53`).

Left: the owner's own `dash.toml` — nothing pairs anything yet, so no board has drawn a real
difference — and then the bench and the car.

## 6. Done when

- `cargo test --workspace` green, including: the builder refuses each of the four bad
  `setpoint`s; a values row with a deviation steps its face down; a chart with a setpoint has no
  seconds; a drift rule does not trip before `hold_ms` and does not trip under `min_setpoint`.
- The firmware builds with a real plan, and `dashsim --preview` shows a values page and a chart
  with and without a setpoint.
- On the car: the owner's numbers for `percent`, `hold_ms` and `min_setpoint`, and a look at
  what boost's difference does on a real pull ([`17-bench-ble-usb.md`](17-bench-ble-usb.md) §4).

## 7. Not in this task

- Pairs across two units (two exchanges, two moments).
- A second trace for the specified value on the chart. Offered and not taken (2026-09-14);
  reconsider once the difference has been watched on the car.
- Guessing pairs from the label data.
