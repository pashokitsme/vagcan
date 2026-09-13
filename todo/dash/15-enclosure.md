# dash / 15 — the enclosure, redesigned around the new CAN module

**Subsystem:** dash · **Needs the car:** no (the printer, and the parts on the desk) ·
**Opened 2026-09-13** · **Hand-off document: written to start a fresh agent session.**

## 0. Read this first

You are designing a 3D-printed enclosure for a small device: a 3.12″ OLED strip that
sits in a car's air vent, plugged into the OBD socket, showing live engine numbers. The
electronics work — the board read the car on 2026-09-13 and passed the bench as a CAN
adapter the same day. What does not exist is a housing the owner is happy with.

The owner speaks Russian, briefly. Talk to them in Russian; write files in English, as
the rest of the repository does. They print on a **Bambu A1**.

**Why this task exists, in the owner's words (2026-09-13):** the CAN module was replaced,
so a new housing is needed; the previous architecture's layout was "very complicated", and
**the boards do not snap in**. Those two complaints are the brief. A design that is merely
smaller, or merely prettier, has not answered them.

## 1. What the previous design was, and what to keep from it

All of it lives in `research/dash/frame/` — **gitignored on purpose** ("one owner's
printed part: FreeCAD sources and their STL/STEP/PNG exports stay local", `.gitignore`).
Read the files; do not try to commit them.

- `frame.py` — one FreeCAD script, three variants, every dimension a named parameter at
  the top. Headless:
  `LC_ALL=en_US.UTF-8 /Applications/FreeCAD.app/Contents/Resources/bin/freecadcmd frame.py`
  (FreeCAD 1.1.3 is installed). Writes `.FCStd` (frame plus the boards as reference
  bodies), `.stl`, `.step`, and four TechDraw SVG views.
- `preview.py` — wraps those SVGs and rasterises them to PNG with `qlmanage`, so the
  views can be looked at without opening FreeCAD. Read the PNGs with the image reader.
- The variants: **`stack`** (three levels, buck / CAN / ESP, ~22 × 24.5 × 16 mm),
  **`flat`** (one plate, boards in a row, ~75 × 24 × 7 mm), **`carrier`** (the flat plate
  grown to the display's footprint, boards on the floor, display pegged onto four corner
  posts and held down by two latch tabs in the end walls, ~103 × 36.5 × 14 mm). `carrier`
  was the one printed.
- `wiring.py` → `wiring.svg`/`.png` — every wire between the boards. **Stale in two
  places**, see §3.

**Keep — these were learned on the printer, and cost prints:**

| lesson | value |
|---|---|
| XY clearance per side for a board in a pocket | **0.1 mm**. 0.35 and 0.2 both printed loose on the A1 |
| Z headroom above the tallest part | 0.5 mm |
| relief pocket under solder bumps, so a board clipped flush does not rock | 0.8 mm deep |
| an 11 mm bridge over a window | prints without support |
| a latch tab's root must not be cut by a window below it | the first `carrier` print had the USB window through the tab root |
| a board must not land on a display corner post | the first print put the ESP on one |
| pegs through a board's mounting holes | 2.9 mm for M3 holes |

**Do not keep:** the idea that the wiring holds the boards in. `flat` and `carrier` were
"open and minimal: no lids, no screws … the wiring is what keeps everything in" (the
script's own docstring). That is exactly the complaint: a board that sits in a pocket on
four L-posts is not *snapped* in. The only snap features were a 0.4 mm ridge on `stack`
and the display's two latches.

## 2. The parts

Dimensions marked *ruler* were taken off photographs with a ruler (the script says so
itself) — **ask the owner to measure with calipers before the first print.** L along the
pin rows, W across, T PCB thickness, H tallest component above the PCB.

| part | size | source | notes |
|---|---|---|---|
| **CAN transceiver module (new)** | **W 10 mm × L 28 mm, 33 mm with its connectors** | owner, 2026-09-13 | per the session record, a fresh SN65HVD230 module ordered as the fix for the counterfeit (two arrived); it carries a 120 Ω terminator (SMD code `121`) that is **removed for the car**, so its pad must stay reachable or be dealt with before assembly. Exact listing, thickness, component height, connector type and edges, mounting holes — **unknown, ask first** |
| ESP32-C3 SuperMini | L 22.5 × W 18.0 × T 1.0, H 3.5 | ruler | USB-C on a short edge, the shell overhangs the PCB edge by ~1.3 mm, 12 mm plug body must pass; two castellated/through-hole pad rows of 8 along the long edges, 3 mm wide; antenna at the far short edge; BOOT (`GPIO9`) and RESET buttons on the top face |
| MP1584EN buck | L 22.0 × W 17.0 × T 1.0, H 4.5 | ruler | four corner pads (IN+, IN−, OUT+, OUT−), 4 mm; trimmed to 3.30 V; its inductor is the tall part |
| SSD1322 3.12″ 256×64 OLED module | PCB 100.5 × 33.5 × 1.0; glass and frame 2.8 above the PCB, parts 2.5 behind it | the module's drawing | four holes on 95.0 × 28.5, 2.25 from the left edge and 3.25 from the right, 2.5 from top and bottom; the flex wraps the bottom edge inside a 64.3 mm notch centred on the board; 2 × 8 header holes at the right end, **wires soldered straight in, no pin header** (it would stand on whatever is below). Whether this module is on the desk yet — ask |
| the old CAN module (gone) | L 14 × W 16 × T 1.6, M3 holes 10.9 apart | ruler | SN65HVD230 "VP230" blue board, a counterfeit chip (`research/dash/can-bring-up.md` §5.3); listed only so its pegs in `frame.py` are recognised as obsolete |

**The front.** The design already settled that the display goes **behind a dark
faceplate** — smoked acrylic, or clear acrylic with tint film — because no OLED is
bezel-less and the filter improves contrast threefold (`todo/dash/README.md`, "The bezel,
and why it does not matter"). The faceplate is part of the printed shell. Whether the
owner has the acrylic, and its thickness — ask.

**The mount.** "Sits in the car's air vent" (`todo/dash/README.md`, first paragraph). No
vent clip has been designed. Which vent, and whether the clip is in scope for this pass —
ask; a housing that takes a separate clip later is a fine answer.

## 3. What changed electrically since `wiring.py`

- **Power comes from OBD pin 1**, +12 V with the ignition only (owner, 2026-09-13;
  `todo/dash/14-one-bus-three-clients.md`, `07`/`08` superseded). `wiring.py` still draws
  pin 16. The chain in the 12 V lead stays: fuse 0.5 A → SS34 → SMBJ20A to GND → buck IN+.
  Ground from pin 4/5.
- **The wake button on `GPIO5` is gone**, and so are the rail divider on `GPIO4` and the
  RS wire to `GPIO3` — the device is simply off when the car is. No button hole for them.
- **Unchanged:** CAN module TX input ← `GPIO6`, RX output → `GPIO1`, 3V3 and GND from the
  buck's rail; CAN-H → OBD 6, CAN-L → OBD 14 as a twisted pair. OLED: CS `GPIO21`,
  RES `GPIO20`, SCLK `GPIO10`, SDIN `GPIO7`, D/C `GPIO0`, VDD 3V3, VSS GND.
- **The cable leaving the housing** carries four conductors to the OBD plug: +12 V (pin 1),
  GND, CAN-H, CAN-L. It needs a strain relief — nothing in the old designs had one.
- **USB-C must stay reachable** with the housing closed. It is how the board is flashed,
  and it is now a product feature: plugged into a laptop, the board becomes a CAN adapter
  (`slcan` image, mode 2 of `14`). A plug body is ~12 × 7 mm.
- **BOOT (`GPIO9`)** is the bench's only button; the car pages the display with the cruise
  lever (`14` §5). A pin-hole to reach BOOT is nice to have, not required.

## 4. The brief

**Done when:** the owner has printed it, every board clicks into place by hand and stays
there with the housing upside down and shaken, the display sits behind the faceplate, the
USB-C takes a cable with the housing closed, and the owner says the layout is simple.

Design goals, in priority order:

1. **Every board snaps in.** A cantilever latch, a barbed clip over a PCB edge, or a
   slot-and-detent — anything that holds the board with no wire and no glue and releases
   it with a fingernail. Prove the fit on a **test coupon** (one pocket, one board, a few
   minutes of print) before the full part; the 0.1 mm lesson above says the A1 is
   unforgiving. Clearance for a snap is not the clearance for a pocket — tune it on the
   coupon, record the value that worked.
2. **A layout a person understands at a glance.** Few levels, boards in the order the
   wires want them (plug → fuse chain → buck → ESP → display; CAN module near the cable
   entry), the wire runs short and visible during assembly. Fewer parts beats clever
   parts.
3. **Assembly order that works.** Wires are soldered before boards go in; the housing must
   let a board with wires already on it drop into its clip. Say the order in the file.
4. **Printable without supports** on the A1, floor on the bed, or say which face needs
   support and why.
5. Then size. The display's 100.5 × 33.5 PCB sets the floor for length and height anyway.

Propose **two or three layouts as a sketch first** (rendered views with the boards as
reference bodies, as `frame.py` does) and let the owner pick before refining one. That is
how the last round worked, and the owner picked `carrier`.

## 5. How to work

- **Same toolchain as before:** a new FreeCAD script beside `frame.py` (e.g.
  `research/dash/frame/housing.py`), every dimension a named parameter at the top with a
  comment saying where it came from (drawing, calipers, ruler, guess), boards as reference
  bodies, STL + STEP + PNG views. The folder is gitignored; that is correct, leave it.
- **No guessed dimension goes into a print unmarked.** The new CAN module's thickness,
  holes and connector positions are unknown today: get them from the owner (calipers, or
  a photo with a ruler in frame) before modelling its clip. A parameter that is a guess
  says `# GUESS` in its comment. This project has a standing rule against invented data;
  it applies to millimetres too.
- **Look at your own renders** before showing them — the image reader works on the PNGs.
  Check that the USB window clears the plug, that no board sits on a post, that no latch
  root is cut.
- **What may be committed:** this task file and its status updates, and a short
  `research/dash/enclosure.md` if the round produces lessons worth keeping (clearances
  that worked, what failed on the printer, with dates). Commit on the branch you are
  given, stage by explicit path, never `git add -A`; never touch `master` or any other
  branch. Commit trailers per `CLAUDE.md` (`Assisted-By:`, `Claude-Session:`).
- **Update `wiring.py`** for §3 (pin 1, no wake button, no divider, the new module's pin
  order once known) in the same folder, and regenerate `wiring.png`.

## 6. Questions for the owner before the first model

1. The new CAN module: part name or listing, thickness, tallest component, connector type
   and on which edges, mounting holes (if any). A photo with a ruler.
2. Is the SSD1322 module on the desk? Calipers on its PCB, holes and header position.
3. The buck: still the MP1584EN, and its measured size?
4. Faceplate: acrylic on hand? thickness? smoked or clear with film?
5. Mount: which vent, and is the vent clip part of this round?
6. How does the cable leave — which end, and does the owner want a connector at the
   housing or a fixed cable?
7. Anything from the printed `carrier` besides "complicated" and "boards don't snap" —
   what was annoying to assemble, what broke, what they liked.
