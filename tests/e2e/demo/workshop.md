# The workshop

The `dev` tools in the shapes the car commands use: `discover` and `sniff` are lists with
their summary first, `glossary` is a card. The bus of the virtual car carries nothing until
it is asked, so `sniff` hears nothing and says what to check.

```console
$ vagcan dev recording discover --log recording.csv --pairs
? success
2 columns · 2 candidates · 0 continuous · 0 never moved

COLUMN               VALUES  CHANGES  READS  SEEN
Coolant temperature       2        1      3  40 41
Engine speed              2        1      3  1000 1200

1 pair changed together

FIRST                SECOND        COINCIDE
Coolant temperature  Engine speed      100%

$ vagcan dev glossary
? success
~/.vagcan/names.csv
0 channels · 0 yours · 0 blank · column en

$ vagcan dev sniff --seconds 1
? success
note: adapter from VAGCAN_DEVICE=[PORT]
type a note + Enter to mark the capture; Ctrl-C stops
     time        id  dir   len   fr  data

[..] s · 0 frames · 0 messages · 0 incomplete · 0 markers · drops none
warning: no traffic on the bus
  (check the ignition, OBD-II pin 6 → CAN-H and pin 14 → CAN-L, and the termination jumper OFF)

$ vagcan dev sniff --seconds 1 --diag-only
? success
note: adapter from VAGCAN_DEVICE=[PORT]
type a note + Enter to mark the capture; Ctrl-C stops
     time        id  dir   len   fr  data

[..] s · 0 frames · 0 diagnostic · 0 messages · 0 incomplete · 0 markers · drops none
warning: no traffic on the bus
  (check the ignition, OBD-II pin 6 → CAN-H and pin 14 → CAN-L, and the termination jumper OFF)

```
