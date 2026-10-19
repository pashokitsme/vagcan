#!/usr/bin/env bash
# One-terminal bench: flash a transmitter to the ESP, then listen on the CANable
# and say whether the board's frames reach the pair. No car. See can-bring-up.md §5.2.
#
#   research/dash/bench.sh            # flash cantx, sniff 15 s, verdict
#   research/dash/bench.sh 30         # sniff 30 s
#   research/dash/bench.sh 15 dash    # flash dash instead of cantx
#
# The bench tools (`cantx`, `cantest`, `rxprobe`) sit behind the firmware's
# `bench` feature so no plain build produces one; this script turns it on. It
# does NOT flash anything back afterwards: a board it leaves with `cantx` floods
# any bus it is plugged into from power-on, so the last thing it prints is the
# reflash command, loudly.
set -u
SECS="${1:-15}"
BIN="${2:-cantx}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FW="$ROOT/crates/dash/vag-dash-fw"
# Where cargo puts what it builds: CARGO_TARGET_DIR when set, as `just` honours it. A path
# fixed to target/ flashed whatever stale image was left there.
ELF="${CARGO_TARGET_DIR:-$FW/target}/riscv32imc-unknown-none-elf/release/$BIN"
VAGCAN="${CARGO_TARGET_DIR:-$ROOT/target}/release/vagcan"

# The CANable has a fixed serial (206E37A...).
CANABLE="$(/bin/ls /dev/cu.usbmodem* 2>/dev/null | grep 206E37A | head -1)"
[ -n "$CANABLE" ] || { echo "no CANable (usbmodem*206E37A*) — plug it in"; exit 1; }

# The ESP as the justfile's `_esp-port` finds it: ESP_PORT, or the one port with
# Espressif's USB vendor id (303a) as `dashsim --boards` lists them, opening no port.
# Never by name: the CANable is a usbmodem too, and this flashes a transmitting image.
if [ -n "${ESP_PORT:-}" ]; then
  ESP="$ESP_PORT"
else
  BOARDS="$(cd "$ROOT/research/dash/host" && cargo run --release -q --bin dashsim -- --boards)" || exit 1
  case "$(grep -c . <<<"$BOARDS" || true)" in
    1) ESP="$BOARDS" ;;
    0)
      echo "no ESP32 board on USB: plug it in (USB before external power); a board that comes and goes: hold BOOT, tap RESET."
      echo "To name its port yourself: ESP_PORT=<port> just bench"
      exit 1 ;;
    *)
      echo "several ESP32 boards on USB; name one: ESP_PORT=<port> just bench"
      sed 's/^/  /' <<<"$BOARDS"
      exit 1 ;;
  esac
fi
[ "$ESP" != "$CANABLE" ] || { echo "ESP_PORT is the CANable's port ($CANABLE): name the ESP32 board's"; exit 1; }
echo "ESP $ESP   CANable $CANABLE   binary $BIN   listen ${SECS}s"

echo "== build + flash $BIN =="
( cd "$FW" && cargo build --release --features bench --bin "$BIN" ) || exit 1
espflash flash --chip esp32c3 --partition-table "$FW/partitions.csv" --port "$ESP" "$ELF" || {
  echo "flash failed — if the port is wedged, hold BOOT on the ESP, replug USB, retry"; exit 1; }

echo "== sniff the pair for ${SECS}s (listen-only, acknowledges nothing) =="
OUT="$(mktemp -t bench).jsonl"
"$VAGCAN" dev sniff --device "$CANABLE" --seconds "$SECS" --out "$OUT"

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

case "$BIN" in
  dash|slcan|rxwatch) ;;
  *)
    echo
    echo "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
    echo "!! THE BOARD STILL RUNS '$BIN' — A BENCH IMAGE. NEVER PLUG IT INTO A CAR. !!"
    echo "!! It drives the pair from power-on. Reflash a car image first:           !!"
    echo "!!   just fw-flash          (or: just fw-flash slcan, the adapter image)  !!"
    echo "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
    ;;
esac
