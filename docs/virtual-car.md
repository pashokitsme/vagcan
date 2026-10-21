# The virtual car

`vagcan dev sim` is a car on a pseudo-terminal. It serves the slcan protocol as a USB-CAN
adapter does, with control units behind it that answer from a profile. Every car command runs
against it unchanged, through `--device`.

It opens no device and touches no bus. The only port it has is the one it makes.

## Running it by hand

```
vagcan dev sim car.toml
```

| Stream | What it says |
|---|---|
| stdout | the port's path, one line: `/dev/ttys012` |
| stderr | what it serves, and how to use it |

```
Serve car.toml on /dev/ttys012
  started 5 units: 7E0 7E1 710 713 70E
Ready until Ctrl-C.
  (use "vagcan info --device /dev/ttys012" in another terminal)
```

In another terminal:

```
vagcan info --device /dev/ttys012
vagcan faults --device /dev/ttys012
vagcan watch --device /dev/ttys012
```

`VAGCAN_DEVICE=/dev/ttys012` stands for `--device` when neither `--device` nor `--ble` is
given; the e2e cases use it. A command that takes it says so on stderr:
`note: adapter from VAGCAN_DEVICE=/dev/ttys012`. A value that is not a serial device is
refused, so a port a closed sim left behind is never opened.

| Flag | What the sim adds on stderr |
|---|---|
| `-v` | each channel opening and closing, refused lines, ISO-TP errors |
| `-vv` | every request (`→`) and answer (`←`), as the client's own `-vv` writes them |

- `vagcan devices` does not list the sim: it lists USB devices.
- Each channel open (`O`) is a new session: counters and series start again.
- The car answers only at 500 kbit/s (`S6`), as diagnostic CAN runs.

## Writing a profile

A profile is TOML. One `[[unit]]` per control unit. Everything a unit says is spelled out;
what is not in the profile is not on the car:

- an identifier the profile lacks is refused with NRC `31`;
- a unit the profile lacks says nothing.

```toml
about = "A made-up car."

[[unit]]
request = "7E0"              # the id it is asked on; it answers on 7E8
name = "engine"              # for the sim's log only
part = "SIM906000A"          # F187
component = "SIM ENGINE"     # F197
odx = "EV_SimEngine"         # F19E
odx_version = "001001"       # F1A2
vin = "SIMVIN00000000001"    # F190

[unit.did]
F189 = { text = "0001" }
0600 = "01 02"
F40D = "00"                  # road speed, km/h: 0 is standing still
F40C = { series = [ { at_ms = 0, value = "0F A0" }, { at_ms = 1000, value = "10 00" } ], loop_ms = 2000 }

[[unit.fault]]
code = "00 01 29"
status = 0x09
extended = "01 06 03 00 10"

[[unit]]
request = "710"
installed = ["713", "70E"]   # the gateway's installation list

[[unit]]
request = "70E"
silent = true                # on the list, never answers
```

### A unit

| Key | What it is |
|---|---|
| `request` | The request id, hex: `"7E0"` or `0x7E0`. The answer id follows the client's rule: ISO 15765-4's `7E0`–`7E7` answer `+8`, VW's `700`–`795` answer `+6A` |
| `name` | A name in the sim's log |
| `part`, `component`, `odx`, `odx_version`, `vin` | `F187`, `F197`, `F19E`, `F1A2`, `F190`, as text |
| `installed` | Units this one lists as fitted: the installation-list bitmap at `2A26` |
| `delay_ms` | Milliseconds before every answer |
| `silent` | Never answers |
| `quiet_after` | Answers this many requests in a session, then nothing |
| `quiet_after_ms` | Answers for this long after the channel opens, then nothing |
| `sessions` | Sessions `10` accepts. Default `[1, 3]` |
| `dtc_availability` | The mask a `19` answer carries. Default `0xFF` |
| `supported` | Extra codes `19 0A` lists, with status `00` |

A shorthand and the same identifier under `[unit.did]` is an error.

### An identifier

Under `[unit.did]`, by its four hex digits. A string is the value as hex bytes. The table
form says one of `value`, `text`, `series`, or `nrc` alone:

| Key | What it does |
|---|---|
| `value` | Hex bytes: `"0F A0"`, spaces optional |
| `text` | Text, sent as its bytes |
| `series` | `[{ at_ms, value }, …]` from the channel opening; each point holds until the next |
| `loop_ms` | The series starts over after this long |
| `nrc` | Refuse every read with this code: `0x22` conditions not correct, `0x33` access denied |
| `pending` | Answer `7F 22 78` (response pending) this many times first |
| `pending_ms` | Milliseconds between those answers. Default 50 |
| `delay_ms` | Milliseconds before the answer |
| `silent_after` | Answer this many reads in a session, then nothing to a read of it |

A read of several identifiers answers the ones the unit has, in the order asked. One the
profile refuses refuses the whole read.

### A fault

| Key | What it is |
|---|---|
| `code` | Three bytes: `"00 01 29"` |
| `status` | Its status byte: `0x09` failed and confirmed, `0x08` confirmed |
| `extended` | What `19 06 <code> FF` answers after the status byte: record number, then data |
| `snapshot` | What `19 04 <code> FF` answers after the status byte |

`19 02 <mask>` lists the faults whose status has a bit of the mask; `19 01` counts them.

### `[bus]`

| Key | Default | What it is |
|---|---|---|
| `padding` | `0x00` | The byte short frames are padded with |
| `block_size` | `0` | The block size the sim's flow control asks for on a long request |
| `st_min` | `0` | The separation time it asks for |

### Behaviours to test

| To test | Write |
|---|---|
| A moving car | engine `F40D = "1E"` (30 km/h): the sweep and `--extended` are refused |
| A slow unit | `pending = 2` on an identifier, or `delay_ms` |
| A refusal | `nrc = 0x22` |
| A unit gone mid-run | `quiet_after = 3`, or `quiet_after_ms` |
| An identifier that stops answering | `silent_after = 1` |
| A unit on the list that is not there | `silent = true` |

## Profiles from what you have

`--from` writes a profile on stdout, from one or more files, merged in order: an earlier
file wins where two say the same.

```
vagcan dev sim --from ~/.vagcan/cars/<VIN>/units.json > car.toml
vagcan dev sim --from drive.csv --vin <VIN> > drive.toml
vagcan dev sim --from units.json --from capture.jsonl > car.toml
```

| File | What it gives |
|---|---|
| a car's `units.json` | every unit's part number, ODX name and version, component; the gateway's list |
| a `dev sniff --out` capture | every identifier read, as a value or a series; fault lists and records |
| a `watch --out` recording | every column a channel matches, turned back into bytes; needs `--vin` and the car's `units.json` |

A capture's reads of several identifiers at once are left out: their answer does not say
where one ends. `-v` counts them.

## The e2e cases

`tests/e2e/<car>/` holds one car: `car.toml` is its profile, and every `*.md` and `*.toml` file
beside it is a [trycmd](https://docs.rs/trycmd) case. For each case file the harness
(`crates/cli/vag-cli/tests/e2e.rs`) starts a sim of that car in-process, makes a fresh
`~/.vagcan` holding a made-up project, and sets `VAGCAN_DEVICE`, so a case reads `$ vagcan
faults`.

| Recipe | What it does |
|---|---|
| `just e2e` | Builds the installed `vagcan` and runs every case. A mismatch prints a diff |
| `just e2e trycmd=units` | Only the case files whose names contain `units` |
| `just e2e-bless` | Rewrites what the cases expect with what the commands print now |

`cargo test` alone does not run them: its `vagcan` is a test build. `just check` and CI do.

A `.md` case is a terminal session. `$ ` starts a command, `? ` its exit status, and the
lines after it are stdout and stderr together:

~~~md
```console
$ vagcan units
? success
names    SIM00 label files
...
```
~~~

A `.toml` case is one command with its streams apart: `streams.stdout` and `streams.stderr`
beside it. Use one where it matters that data is on stdout alone.

| In a case | Matches |
|---|---|
| `[..]` | any run of characters within one line |
| `...` on a line of its own | any lines |
| `[PORT]` | the sim's port |
| `[HOME]` | the run's `~/.vagcan` |
| `[VERSION]` | the binary's version |
| `<name>.in/` | the case's working directory: files a command reads |

`just e2e-bless` keeps an elision whose neighbours did not change; one next to a changed line can
come back as the literal text. Read the diff and put it back before committing.

What moves between runs is elided where it stands, never filtered away: `-vv`'s response
times are `[..] ms`, `watch`'s rows are `...`.

- **Width.** `VAGCAN_COLUMNS=80 vagcan faults` lays the table out for 80 columns, as a terminal
  that wide would. Set it in the case, never in the harness: a variable the harness sets wins
  over the case's.
- **Another port.** `--device <path>`, not `VAGCAN_DEVICE=`, for the same reason.
- **Data.** Profiles, part numbers, ODX names, channels and fault texts in `tests/e2e/` are
  made up. Never put a real car's or Ross-Tech's data there.

A change to a command's output is checked with `just e2e`, blessed with `just e2e-bless`, and
its case diff shown in the PR (CLAUDE.md, "Command output").
