# dash / 12 — settings that survive the ignition

**Subsystem:** dash · **Needs the car:** no · **Working on hardware 2026-08-25**

A configuration that does not survive the ignition being switched off is not a
configuration. This is where it is kept and how it gets there; the link that carries it
is [`11-ble.md`](11-ble.md).

## What a setting is, and what it is not

The catalogs are **flashed**, as part of a firmware built for one car. The board decodes
nothing at run time and has nowhere near the memory to — `README.md`'s "the device
resolves nothing, it executes a plan".

So a setting selects **among what is already in the image**: which pages exist, what kind
each is, which cells it shows, which page comes up at boot, brightness. A cell is an
**index into the flashed plan**, not an identifier. That is not a guard bolted on; it is
the only thing the type can express, so a forty-first identifier is not refused, it is
unsayable.

```rust
struct Config {
  brightness: u8,
  active_page: u8,
  pages: heapless::Vec<Page, 8>,     // Page { kind: Chart|Values, cells: Vec<u16, 8> }
}
```

`SCHEMA_VERSION` is stored beside it. A blob written under a different version is
**ignored, never reinterpreted** — a configuration read under the wrong schema is worse
than no configuration, because it is confidently wrong rather than absent.

## Where it lives: a partition found, not an address written

`partitions.csv` adds a `config` partition. The default table has `factory` filling the
flash to the last byte, so room has to be made rather than found: `factory` is cut to
3 MB (the image is 351 KB) and `config` takes 32 KB after it.

```
nvs,        data, nvs,       0x9000,   0x6000,
phy_init,   data, phy,       0xf000,   0x1000,
factory,    app,  factory,   0x10000,  0x300000,
config,     data, undefined, ,         0x8000,
```

**No offset appears in the source.** `esp-bootloader-esp-idf` reads the partition table
from flash at start-up and the partition is located **by label**; `esp-storage` 0.7 then
does the reading and writing. A constant in the code would be right until the first time
this file changed, and then wrong in a way that corrupts rather than fails.

(0.7 is deliberate: it is the last release with no `esp-hal` dependency of its own.
0.8 and later want `esp-hal 1.0.0-rc.1` and the firmware is on rc.0.)

## How it survives a power cut

Two slots, one flash sector each (4096 B, the erase granularity — a slot smaller than a
sector could not be rewritten without disturbing its neighbour), and a generation
counter.

```
magic "VDSH" | schema version | payload len | generation | CRC-32 | postcard payload
```

A save always writes the slot that is **not** current; it then becomes current by holding
the higher generation. Lose power mid-write and the damaged slot is the *old* one — the
previous configuration is still whole in the other, and there is no instant at which both
are invalid. Loading reads both, discards whatever fails magic, version or CRC, and takes
the survivor with the higher generation.

The CRC is what makes "damaged" detectable at all: erased flash reads as `0xff`, which is
a perfectly plausible-looking blob to anything that does not check.

`Config::validate()` runs **before** the write, not after the read: an unusable
configuration must never reach flash, because the device has to be able to trust what it
reads at boot. And `load()` reports `Empty` rather than quietly substituting defaults —
"never saved" and "saved these defaults" are different facts, and only one of them is a
bug.

## What was verified on the board

Over BLE, using [`research/dash/bleecho`](../../research/dash/bleecho):

```
> get
< brightness 128 page 0 of 2 | 0:values[0, 1, 2, 3] | 1:chart[0] | saved gen 0
> set brightness 42
> set page 1
> save
< ok: saved, generation 1
```

- **Found at run time:** `config partition at 0x310000, 32768 bytes (8192 in use: 2 slots)`.
- **Survives a reset:** after a chip reset the boot log reads
  `config loaded, generation 1: Config { brightness: 42, active_page: 1, … }`.
- **Survives a firmware reflash.** `espflash` writes bootloader, partition table and app;
  the `config` partition is untouched, so settings outlive a firmware update. Verified by
  reflashing and reading the same generation back.
- **Slots alternate.** Three saves in a row produced generations 1, 2, 3, each loading
  correctly after a reset.

What is **not** verified, and cannot be from the bench: the rollback itself. Interrupting
a write requires cutting power mid-sector. The property holds by construction and the
newest-of-two selection is exercised by the alternation above, but it has not been
provoked. Say so rather than claim it.

## One way to lose the partition table

**`espflash flash` without `--partition-table` silently writes the default table back.**
It happened here: flashing an unrelated Wi-Fi binary reverted the table, and the `config`
entry vanished from it — the firmware would then report `NoPartition` and forget every
setting, on a board whose settings were still physically intact at 0x310000.

Reflashing *with* the table restored it and generation 3 read back unchanged, so the data
survives; but a partition table is part of the image, not of the board, and every flash
of this project must carry it. That is an argument for the flash command living in
`.cargo/config.toml` as a runner rather than being typed.

## The command surface, and why it is text

The first form of the protocol is lines of ASCII over the Nordic UART Service:

```
get | set brightness N | set page N | save | load | defaults | erase
```

A binary framing with a CRC is what the real client wants and `11` describes it. Text is
what a person with a terminal can drive by hand, and right now being able to drive it by
hand is worth more than parsing it fast. `get` reports `UNSAVED` when memory and flash
disagree, so "did that survive?" is answered by looking rather than by rebooting.

## Next

- **The client.** A TUI first, over BLE, from `vagcan`; a phone application only if it
  turns out to be wanted. No bonding: the client is a program of ours, so there is no
  operating-system pairing dialogue to satisfy.
- **Binary framing** once the client exists — 244-byte frames, kind and *n* of *m*, CRC
  over the whole blob. Reliability and ordering come free from `write with response`.
- **Schema migration.** Today a version bump discards the stored settings. That is the
  right default and the wrong end state; a migration path is owed before the device is
  ever in someone else's hands.
- **Wear.** Not a concern at human write rates — one sector per save, alternating, on
  flash rated for tens of thousands of erases — but it becomes one the moment anything
  automatic starts saving. Nothing automatic may save.
