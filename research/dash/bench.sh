#!/usr/bin/env bash
# One-terminal bench: flash a transmitter to the ESP, then listen on the CANable
# and say whether the board's frames reach the pair. No car. See can-bring-up.md §5.2.
#
#   research/dash/bench.sh            # flash cantx, sniff 15 s, verdict
#   research/dash/bench.sh 30         # sniff 30 s
#   research/dash/bench.sh 15 dash    # flash dash instead of cantx
set -u
SECS="${1:-15}"
BIN="${2:-cantx}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FW="$ROOT/crates/dash/vag-dash-fw"
ELF="$FW/target/riscv32imc-unknown-none-elf/release/$BIN"

# The CANable has a fixed serial (206E37A...); the ESP is the other usbmodem and
# re-enumerates, so it is found by exclusion.
CANABLE="$(/bin/ls /dev/cu.usbmodem* 2>/dev/null | grep 206E37A | head -1)"
ESP="$(/bin/ls /dev/cu.usbmodem* 2>/dev/null | grep -v 206E37A | head -1)"
[ -n "$CANABLE" ] || { echo "no CANable (usbmodem*206E37A*) — plug it in"; exit 1; }
[ -n "$ESP" ]     || { echo "no ESP usbmodem port — plug it in, or hold BOOT and replug if wedged"; exit 1; }
echo "ESP $ESP   CANable $CANABLE   binary $BIN   listen ${SECS}s"

echo "== build + flash $BIN =="
( cd "$FW" && cargo build --release --bin "$BIN" ) || exit 1
espflash flash --chip esp32c3 --partition-table "$FW/partitions.csv" --port "$ESP" "$ELF" || {
  echo "flash failed — if the port is wedged, hold BOOT on the ESP, replug USB, retry"; exit 1; }

echo "== sniff the pair for ${SECS}s (listen-only, acknowledges nothing) =="
OUT="$(mktemp -t bench).jsonl"
"$ROOT/target/release/vagcan" dev sniff --device "$CANABLE" --seconds "$SECS" --out "$OUT"

FROM_BOARD=$(grep -c '"Standard":2016' "$OUT" 2>/dev/null); FROM_BOARD=${FROM_BOARD:-0}   # 0x7E0 = 2016
ANY=$(grep -c 'CanFrame' "$OUT" 2>/dev/null); ANY=${ANY:-0}
echo
echo "== verdict =="
echo "frames on the pair: $ANY   from the board (7E0): $FROM_BOARD   capture: $OUT"
if [ "$FROM_BOARD" -gt 0 ]; then
  echo "PASS — the board's frames reach the pair. The transmit path is fixed."
elif [ "$ANY" -gt 0 ]; then
  echo "partial — other traffic seen but no 7E0. The board still is not heard."
else
  echo "FAIL — nothing reached the CANable. Board transmit still broken (or the CANable is not on the pair)."
fi
