# A unit that falls silent under a read

The brakes answer one request and then say nothing. A read of them stops after a quiet run of
timeouts, with the STOPPED notice, rather than waiting out every identifier. The
identification block asks `F187` first: it gets the one answer, and the block stops three
timeouts later. `sensors` gets a refusal for its first parameter and stops the same way.

```console
$ vagcan units --identify 713
? failed
note: adapter from VAGCAN_DEVICE=[PORT]

STOPPED: control unit 713 (713) stopped answering altogether — 3 requests in a row went unanswered.
The tool was asking it for identifier F102.

The read has ended here. A control unit that changes under a sweep may have
stopped doing its job in the car, and a sweep that carries on past that is
how a recoverable fault becomes a lasting one.

What to do now:

1. Stop the car if it is moving.
2. Do NOT clear the faults. The freeze frame is the evidence.
     vagcan faults --unit 713 --details
3. See whether it answers at all. This asks every unit of the car the four
   identifiers that name it, 713 among them — no sweep:
     vagcan units --identify
4. Try an ignition cycle: off, wait, on. A unit that crashed and restarted
   often comes back. One that does not is a different problem.
5. Then stop, and take it to someone with the factory tool.
error: the read was stopped: control unit 713 changed while it was being read

$ vagcan sensors --unit 713
? failed
note: adapter from VAGCAN_DEVICE=[PORT]

STOPPED: control unit 713 (713) stopped answering altogether — 3 requests in a row went unanswered.
The tool was asking it for identifier F407.

The read has ended here. A control unit that changes under a sweep may have
stopped doing its job in the car, and a sweep that carries on past that is
how a recoverable fault becomes a lasting one.

What to do now:

1. Stop the car if it is moving.
2. Do NOT clear the faults. The freeze frame is the evidence.
     vagcan faults --unit 713 --details
3. See whether it answers at all. This asks every unit of the car the four
   identifiers that name it, 713 among them — no sweep:
     vagcan units --identify
4. Try an ignition cycle: off, wait, on. A unit that crashed and restarted
   often comes back. One that does not is a different problem.
5. Then stop, and take it to someone with the factory tool.
error: the read was stopped: control unit 713 changed while it was being read

```
