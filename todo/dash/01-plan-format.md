# dash / 01 — the plan, and the generator that writes it

**Subsystem:** dash · **Crate:** `vagcan` (generator), `vag-dash` (the type) · **Needs the car:** no

## Goal

Define what a dash plan *is*, and make `vagcan dash build` produce one from the catalogs
for a named VIN.

The plan is the whole interface between this project and the device. Everything the
firmware knows about the car arrives through it; everything it cannot do is a
consequence of what the plan cannot express.

## The shape

```
Plan {
  vin:      String,              // baked so the firmware can check where it is
  units:    [ Unit { address: u16, part_number: String } ],
  pages:    [ Page ],
  alarms:   [ Alarm ],           // see 04-alarms.md
  language: En | Ru,
}

Page = Chart { channel: Channel, min: f32, max: f32 }
     | Values { title: String, cells: [Channel; 1..=4] }

Channel {
  unit:       u16,               // 0x7E0, 0x7E1 …
  did:        u16,
  bit_offset: u32,               // bits into the response after `62 hi lo`
  bit_length: u32,
  signed:     bool,
  big_endian: bool,              // stored, never assumed — 0x380A is little-endian
  factor:     f32,
  offset:     f32,
  decimals:   u8,
  unit_text:  String,            // "°C", "bar", "Nm"
  label:      String,            // ALREADY RENDERED, in `language`
}
```

Note what is *absent*: no text ids, no glossary, no variant names, no scaling
expressions, no lookup of any kind. Those are build-time concepts. Anything the firmware
would have to resolve is a bug in this task.

## Where the data comes from

- The channel rows from `reading` in `~/.vagcan/data/<project>/cache.sqlite`, selected by
  the variant the car reports (`F19E` + the first three digits of `F1A2`, via
  `vag_data::label_files::odx_match`) — the same resolution `watch` already does.
- **A proven measurement outranks a declared one**, exactly as everywhere else in this
  project: `~/.vagcan/data/<project>/measurements/<part number>.json` first, the catalog
  second. Where the two disagree the plan carries the proven scaling and says so in the
  build log.
- The label text through the owner's own glossary (`~/.vagcan/names.csv`) first, the
  vendor name second — `extracted::name_of` already implements that order.
- `min`/`max` for chart pages: from the catalog where it gives a range, otherwise stated
  by hand in the build input. **Autoscale is not an option** — see `02`.

## Which pages, and from where

The build input is a small TOML written by hand for now, naming the pages and their
channels by text id or by `unit:did:offset`. The owner's `config.toml` already holds
`[favourites]` and `[charted]` per VIN, written by `watch`'s settings screen; wiring
those in as a *default* build input is the follow-up, not this task.

## Output

Two forms, same content:

- `plan.json` — for the simulator (`03`) and for a human to read.
- `plan.rs` — a `const` the firmware `include!`s, so the device needs no filesystem at
  all and has no state in which the plan is missing or corrupt.

Both are written under `~/.vagcan/dash/<vin>/` and **must not be committed**. They are
derived from VW's and Ross-Tech's data. Add the path to `.gitignore` in the same commit
that first writes one.

## Refusals that belong here

- The generator emits `0x22` reads and nothing else. If a build input names a channel
  that would require any other service, it fails the build with that as the reason.
- A channel that the resolved variant does not declare is a build error, not a warning.
  The plan's value is that it cannot ask for something unproven; a plan that quietly
  carried a guess would give that away.

## Tests

- A plan built from a fixture catalog round-trips JSON → `Plan` → JSON unchanged.
- A little-endian row (`0x380A`) survives the build with `big_endian: false`. Assert the
  decoded value, not the flag: a reader that assumed big-endian reports 690 /min as
  45570, and that is the bug this column exists to prevent.
- A sub-byte field (`bit_length < 8`) keeps its offset in **bits**, and two fields of one
  identifier produce two channels rather than one.
- A proven measurement beats a declared one for the same channel.
- Building for a VIN whose variant lacks a named channel fails, and the message names the
  channel.
- **No test writes into the owner's own `~/.vagcan`.** `watch/favourites.rs` carries the
  self-checking test that enforces this; do the same here.

## Done when

`vagcan dash build --vin <VIN> --input pages.toml` writes a `plan.json` whose every
channel is one the catalog declares for this car, and `03`'s simulator renders from it.
