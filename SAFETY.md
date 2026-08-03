# Safety

This tool only reads. It has still cost one car its power steering.

Read this before pointing it at a car you care about. It is short, and every
rule in it was paid for.

---

## The incident

On 2 August 2026 the reference car — a Škoda Octavia III on MQB — lost power
steering assist during a `vagcan survey` run. The sequence, from the owner:

1. First sweep, car parked with the ignition on: assist dropped out.
2. Engine off, engine on: **assist came back**. The car drove normally.
3. A kilometre later, second sweep, this time with the car moving: assist
   dropped out again — at speed.
4. Restart did not help. It has not come back since.

The steering assist control unit now stores `B2000` (control unit defective —
internal memory checksum), `B200F` (internal fault) and `B1168` (steering
angle: no initialisation). Its warning lamp is on. It answers every diagnostic
request normally and its identification block is intact — it is not dead, it is
refusing to assist. The commissioning dataset at identifier `1923` read
`01 00 28 61 D7 E9` before and reads `01 00 00 00 00 00` after, and did not
refill over a subsequent drive. Restoring it needs a factory procedure this
tool cannot and will not perform.

The full write-up is `research/eps-j500-report-ru.md`; the technical detail is
in `research/whole-car-survey.md` §3b.

---

## Why a read-only tool could do that

Two mechanisms, both worth understanding before you dismiss them.

**An identifier sweep is a fuzz test.** `survey` asks a control unit for 2816
identifiers, most of which nothing has ever asked it for. Each one takes a path
through its diagnostic server, and a path with a defect in it crashes the
server — which on this car meant the task that provides steering assist. VW's
own bulletin **TPI 2055045/4** (address 44, May 2024) documents exactly this
code appearing *"during maintenance or repair work"*, with the technical reason
*"wrong software for the power steering control unit"*. The factory knows.

**An extended diagnostic session is workshop mode.** `0x10 0x03` tells a unit
that someone is working on the car. A unit whose job is to assist the driver is
entitled to stop assisting while it is in one, and several do. This is not a
malfunction; it is the designed behaviour, and it is why VCDS warns you before
opening one on a moving car.

Neither of these writes a single byte. "Read-only" bounds what you can *change*
about a car. It does not bound what you can *provoke*.

---

## What the tool does about it now

* The extended session is **not sent** by any command by default. `survey` and
  `faults` both read fine without one.
* `--extended` is **refused while the car is moving**, established by reading
  road speed from the engine first. A car that will not report its speed counts
  as moving.
* **Both** sweeps — `survey` over every unit and `scan` over one — are **refused
  while the car is moving** unless `--while-driving` is passed, and the refusal
  explains what happened here rather than citing a rule. `scan` went unguarded
  at first for no better reason than that the incident happened during a survey;
  they are the same operation over a different number of units, and guarding one
  only moves the danger to the other spelling. A test now asserts that every
  sweep has the gate, so the next one cannot ship without it (2026-08-03).
* Nothing in the UDS client will emit a write service. The allowlist admits
  `0x22` (read data), `0x19` (read faults), `0x10` (session) and `0x3E`
  (tester present), and that is the whole of it.

None of this makes a sweep safe. It makes a crash *survivable* — recoverable by
stopping and restarting, which is what the first drop-out turned out to be, and
the second was not.

---

## Rules

**Sweep parked.** A control unit that falls over while you are stationary is an
inconvenience. The same event at 60 km/h is not. Restart the engine and the
unit usually comes back; that option does not exist mid-corner.

**Sweep one unit at a time when you can.** `--only 713` costs seconds and tells
you which unit misbehaved. A whole-car pass gives you a list of suspects.

**Snapshot before you sweep something new.** `survey --only <unit> --out
before.jsonl`. If the unit later misbehaves, `survey --diff` shows exactly what
changed, in bytes. On this car that is how the lost dataset was found — nobody
would have noticed six bytes going to zero by eye.

**Treat the steering, brakes, airbag and gateway as the units that matter.**
They are the ones whose misbehaviour reaches the driver. There is no rule that
they are more fragile; there is a rule about what it costs when they are.

**Stop when something changes.** If a lamp comes on, or a system goes quiet,
finish nothing and start nothing. Read the faults, write down what you were
doing, and stop. The second sweep on this car happened because the first
drop-out looked like it had resolved itself.

**Have the car's own scan from before.** The reference car's earlier VCDS scan
turned out to be decisive: it showed the steering unit was already logging
complaints about supply voltage and already limiting assist, days before
anything of ours touched it. Without that, the whole incident would have read
as purely self-inflicted. Keep a baseline.

**Do not add write support.** Not coding, not adaptation, not clearing faults,
not flashing. Those are jobs for a tool with the manufacturer's security access
and the manufacturer's data — and the one failure this project has caused was
in a system where a wrong write is not a bad file, it is a car that does not
steer the way the driver expects.

---

## If a unit stops behaving

1. **Stop the car.** Then stop the tool.
2. **Do not clear the faults.** The freeze frame is the evidence — mileage, the
   car's own timestamp, occurrence count, supply voltage, and on some units the
   internal state at the moment it failed. Clearing costs you the diagnosis.
   Read them: `vagcan faults --ecu <unit> --details`.
3. **Take a snapshot**: `vagcan survey --only <unit> --out after.jsonl`, then
   `vagcan survey --diff before.jsonl after.jsonl`. Bytes that changed and
   stayed changed are what actually happened.
4. **Try an ignition cycle.** Off, wait, on. A unit that crashed and restarted
   often comes back. One that does not is a different problem.
5. **Then stop and go to someone with the factory tool.** Ross-Tech's own
   forums, VW's bulletins and the workshop diagnosis are worth more at that
   point than another read.

---

## What this is not

This is not a reason to avoid reading cars. Everything else in this project —
the identification of fifteen control units, 1206 identifiers, the fault codes
with their dates, the measurement scalings proven live — came from reading, and
none of it did any harm.

It is a reason to know which of your reads is the dangerous one. On this
project it is exactly one command, and it is now the only one that argues back.
