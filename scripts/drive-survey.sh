#!/usr/bin/env bash
#
# One driving pass of `vagcan survey`, then the comparison against a parked
# pass. What comes out is the list of identifiers whose bytes differ between
# the two conditions — which is the list of live measurements, obtained
# without a label file, without VCDS, and without guessing.
#
# Usage:
#   scripts/drive-survey.sh                 # compares against the newest parked pass
#   scripts/drive-survey.sh parked.jsonl    # or against a specific one
#
# Read-only: the only services issued are reads. Start it, put the phone down,
# drive.

set -euo pipefail
cd "$(dirname "$0")/.."

PARKED="${1:-research/dumps/survey-parked.jsonl}"
STAMP="$(date +%Y%m%d-%H%M)"
OUT="research/dumps/survey-driving-${STAMP}.jsonl"
REPORT="research/dumps/survey-diff-${STAMP}.txt"

if [ ! -f "$PARKED" ]; then
    echo "No parked survey at $PARKED."
    echo "Record one first, with the ignition on and the car standing still:"
    echo "    cargo run -q -p vagcan -- survey --out $PARKED"
    exit 1
fi

cat <<'BRIEF'
Driving survey — about 8 minutes.

While it runs, give the car things to say. Each one moves a different unit,
and anything that never moves proves nothing:

    speed          accelerate to ~60+ and slow down again, more than once
    brakes         several firm stops, and the handbrake once while stopped
    steering       full lock each way at low speed
    indicators     left for a while, then right
    lights         dipped on, main beam once, hazards for ten seconds
    doors          keep them shut; the door modules report the closed state
    climate        change the temperature and the fan speed
    gearbox        let it change up and down; use S or the paddles once

Nothing here writes to the car. Press Ctrl-C at any time — every unit that
finished is already saved.

BRIEF

read -r -p "Ready? Start the drive, then press Enter. " _

echo
echo "recording to $OUT"
cargo run -q -p vagcan -- survey --out "$OUT"

echo
echo "comparing with $PARKED"
cargo run -q -p vagcan -- survey --diff "$PARKED" "$OUT" | tee "$REPORT"

echo
echo "saved: $OUT"
echo "       $REPORT"
echo
echo "Next: watch the ones that moved, live —"
echo "    cargo run -q -p vagcan -- watch --survey $OUT"
echo "and inside it press 'c', then '/' to filter by unit or identifier."
