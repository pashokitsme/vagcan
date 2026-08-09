# Safety

This tool only reads. It has still cost one car its power steering, and a week
later it came close to doing it again — to the same car, parked.

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

The full write-up is `research/eps/eps-j500-report-ru.md`; the technical detail is
in `research/car/whole-car-survey.md` §3b.

## The second one

On 9 August 2026 a `vagcan survey` run **nearly did it again**, to the same
car. From the owner: *"vagcan survey чуть не убил мне рейку ещё раз — машина
была припаркована. Он высветил ошибку, но она потом погасла."* — it nearly
killed the rack a second time; the car was parked; a warning appeared, and then
it went out.

Parked. Every rule in the section below was in force, and all of them were
about *where* the car was. None of them was about what the sweep was asking, or
about what it did when the answers changed. Three things were wrong:

1. **The sweep asked blind.** Every unit got the same 2816 identifiers, with no
   evidence any of them existed — including the steering assist, which
   `EV_SteerAssisMQB` declares 161 identifiers for. The other 2655 were the
   fuzz test.
2. **Nothing stopped when something changed.** "Stop when something changes"
   was written in this file and not implemented anywhere in the code. A unit
   that went silent mid-sweep was counted as a failed read and the sweep
   carried on — to the next identifier, and then to the next unit. It noticed
   and continued, which is worse than not noticing.
3. **The warning was erased.** Progress was reported on a line that rewrites
   itself. Anything printed during a sweep shared that line and was gone at the
   next redraw. That is "it showed an error, and then it went out."

What changed as a result is the section below. This entry stays because the
first incident's entry stays: the rules here are not deductions, they are
receipts.

---

## Why a read-only tool could do that

Two mechanisms, both worth understanding before you dismiss them.

**An identifier sweep is a fuzz test.** A sweep asks a control unit for
identifiers nothing has ever asked it for. Each one takes a path through its
diagnostic server, and a path with a defect in it crashes the server — which on
this car meant the task that provides steering assist. VW's own bulletin **TPI
2055045/4** (address 44, May 2024) documents exactly this code appearing
*"during maintenance or repair work"*, with the technical reason *"wrong
software for the power steering control unit"*. The factory knows.

This is the mechanism, and after 9 August 2026 it is no longer what the tool
does by default — see below.

**An extended diagnostic session is workshop mode.** `0x10 0x03` tells a unit
that someone is working on the car. A unit whose job is to assist the driver is
entitled to stop assisting while it is in one, and several do. This is not a
malfunction; it is the designed behaviour, and it is why VCDS warns you before
opening one on a moving car.

Neither of these writes a single byte. "Read-only" bounds what you can *change*
about a car. It does not bound what you can *provoke*.

---

## What the tool does about it now

**It does not sweep blind.** `survey` and `scan` ask a control unit only the
identifiers some source says that unit answers. The unit reports what it is —
`F187` its part number, `F19E` the ODX file it names, `F1A2` the version — and
that resolves to an ODIS variant which *declares which identifiers it defines*.
The steering assist on this car declares 161. It used to be asked 2816.

Asking a control unit the questions its own manufacturer's data says it answers
is not a fuzz test. Blind sweeping was once the only way to find out what a unit
answered; it is not any more, so it is no longer what happens when you type
`vagcan survey`.

* **A unit no source describes is identified, not swept.** Two of this car's
  fifteen resolve to no variant. They get their identification block and their
  fault memory read, and nothing else — the old behaviour swept exactly those
  units the hardest, which is the fuzz test aimed at the units the tool
  understands least.
* **Blind sweeping is `--blind`, aimed at units named one at a time.** There is
  no value of any flag that means "sweep the whole car blind". `survey --blind`
  with no unit list is a parse error, on purpose: that was the default, and it
  is what did the damage. `--range` describes a blind sweep and is refused
  without one rather than quietly ignored. The flag's help says what it costs,
  in the words above.

**It stops when something changes.** The rule below was written here after the
first incident and lived nowhere else; it is now in the code. Every sweep
carries a watchdog — there is no spelling of the sweep function without one —
and the run **ends**, non-zero, the first time either of these happens:

* a unit that had been answering goes quiet (three unanswered requests in a
  row), or
* it goes back on an identifier it already answered in that same run. Because
  most of an identifier space is refusals, and a unit that has fallen over
  looks exactly like one that implements nothing here, a known-good identifier
  is re-read every 64 requests specifically to tell those apart.

It ends the **whole run**, not that unit: what made the second drop-out
permanent was carrying on after the first looked like it had resolved itself.
The tool prints what happened, what it was asking when it happened, and the
steps under "If a unit stops behaving" below. The car's whole-car survey cache
is deliberately *not* written, so the "before" half of the `--diff` comparison
survives; `--out` is written line by line, so a stopped run keeps its evidence.

**A safety message is never written where it can be erased.** Progress goes on
a line that rewrites itself; warnings and halts do not go there. The progress
line is cleared first and the message written whole, on a line nothing redraws
over. A test asserts the ordering.

**The rest, unchanged and still in force:**

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

This does not make a control unit unbreakable. A declared identifier can still
be the one whose path through the firmware has the defect in it, and this car
proves that a parked sweep is not a safe sweep. What has changed is that the
default is no longer an experiment on the car, and that an experiment which goes
wrong stops instead of continuing. The previous version of this section ended
"none of this makes a sweep safe, it makes a crash survivable" — the first
drop-out was survivable, the second was not, and a model that only bets on
recovery is the model that lost.

---

## Rules

**Sweep parked.** A control unit that falls over while you are stationary is an
inconvenience. The same event at 60 km/h is not. Restart the engine and the
unit usually comes back; that option does not exist mid-corner.

**Sweep one unit at a time when you can.** `--only 713` costs seconds and tells
you which unit misbehaved. A whole-car pass gives you a list of suspects.

**`--blind` is the dangerous flag now.** Everything else is reading what the car
says it will answer. `--blind` is the one that asks a control unit questions
nothing has ever asked it, which is the operation this whole file is about.
Aim it at one unit, parked, with a snapshot taken first, and not at the
steering, the brakes, the airbag or the gateway unless you have a reason worth
the rack.

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
drop-out looked like it had resolved itself. The tool now enforces the half of
this it can see — a unit going quiet or going back on itself ends the run — but
it cannot see a lamp on the dashboard, and that is still your job.

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
project it is exactly one flag — `--blind` — and it is the one that argues
back. Everything else asks the car only what the car's own data says it will
answer, and stops if the answers change.
