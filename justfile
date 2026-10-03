# vagcan — build, flash and check. `just` lists the recipes.
#
# Two environment variables steer the firmware recipes:
#   VAGCAN_DASH_VIN  the car whose plan the image is built with; unset = the one car under
#                    ~/.vagcan/dash (the firmware's build.rs decides, and says so)
#   ESP_PORT         the board's serial port; unset = the one /dev/cu.usbmodem* that is not
#                    the CANable (its serial starts 206E37A, as research/dash/bench.sh knows)

set shell := ["bash", "-euo", "pipefail", "-c"]

fw := "crates/dash/vag-dash-fw"
host := "research/dash/host"

# the list of recipes
default:
    @just --list --unsorted

# ---- the firmware (crates/dash/vag-dash-fw) ----------------------------------------------

# build an image: `just fw-build` (dash, BLE), `just fw-build dash noble`, `just fw-build oledtest`
fw-build bin="dash" ble="ble":
    cd {{fw}} && cargo build --release --bin {{bin}} {{ if ble == "noble" { "--no-default-features" } else { "" } }}

# build and flash an image: `just fw-flash`, `just fw-flash dash noble` (a board whose BLE does not start)
fw-flash bin="dash" ble="ble": (fw-build bin ble)
    port="$(just _esp-port)" && cd {{fw}} && espflash flash --chip esp32c3 --partition-table partitions.csv --non-interactive \
        --port "$port" "${CARGO_TARGET_DIR:-target}/riscv32imc-unknown-none-elf/release/{{bin}}"

# build, flash and stay on the board's console (Ctrl-C leaves)
fw-run bin="dash" ble="ble": (fw-build bin ble)
    port="$(just _esp-port)" && cd {{fw}} && espflash flash --chip esp32c3 --partition-table partitions.csv --monitor \
        --port "$port" "${CARGO_TARGET_DIR:-target}/riscv32imc-unknown-none-elf/release/{{bin}}"

# the board's console, without flashing (opening the port resets the board)
fw-monitor:
    port="$(just _esp-port)" && espflash monitor --chip esp32c3 --port "$port"

# which board is on USB: chip revision (v0.4 has no working BLE), MAC
fw-info:
    port="$(just _esp-port)" && espflash board-info --chip esp32c3 --non-interactive --port "$port" | grep -E "Chip type|MAC|Flash size"

# the test picture on the OLED, nothing on CAN: border, `vagcan`, a dim copy, a checkerboard
fw-oledtest: (fw-flash "oledtest")

# the board as a plain slcan adapter for `vagcan --device` (flash `dash` back afterwards)
fw-slcan: (fw-flash "slcan")

# fmt and clippy on both builds, as CI runs them (an empty plan: no car needed)
fw-check:
    cd {{fw}} && cargo fmt -- --check
    cd {{fw}} && VAGCAN_DASH_NO_CAR=1 cargo clippy --release --bins -- -D warnings
    cd {{fw}} && VAGCAN_DASH_NO_CAR=1 cargo clippy --release --bins --no-default-features -- -D warnings

# the static-RAM budget, with and without BLE (CI's `firmware` job)
fw-ram:
    {{fw}}/ram-budget.sh

# The bench image transmits on its own from power-on and `bench.sh` does not flash `dash` back.
# Never leave it on a board that goes into a car: `just fw-flash` after it.

# the CAN bench with the CANable: flashes a TRANSMITTING image (cantx), sniffs, says if frames arrive
bench secs="15" bin="cantx":
    research/dash/bench.sh {{secs}} {{bin}}

# ---- the laptop side ------------------------------------------------------------------------

# build vagcan and vagcan-measure (target/release/)
build:
    cargo build --release -p vag-cli -p vag-cli-measure

# install vagcan and vagcan-measure into ~/.cargo/bin
install:
    cargo install --path crates/cli/vag-cli
    cargo install --path crates/cli/vag-cli-measure

# run vagcan from the checkout: `just vagcan info`, `just vagcan watch --device /dev/cu.usbmodem1101`
vagcan *args:
    cargo run --release -q -p vag-cli -- {{args}}

# the dash plan for a car, offline (~/.vagcan/dash/<VIN>/plan.json and plan.rs): `just plan <VIN>`
plan vin:
    cargo run --release -q -p vag-cli -- dev dash build {{vin}}

# dashcfg, the board's settings over BLE (from Terminal.app: macOS refuses BLE to an agent's shell)
dashcfg *args:
    cargo run --release -q -p vag-dash-cfg -- {{args}}

# a bench-rig tool from research/dash/host: `just host dashsim`, `just host benchecu --bench …`
host bin *args:
    cd {{host}} && cargo run --release -q --bin {{bin}} -- {{args}}

# the board's screen in this terminal, over USB
dashsim *args:
    cd {{host}} && cargo run --release -q --bin dashsim -- {{args}}

# ---- checks ---------------------------------------------------------------------------------

# rustfmt everywhere: the workspace, the bench crate and the firmware
fmt:
    cargo fmt --all
    cd {{host}} && cargo fmt
    cd {{fw}} && cargo fmt

# the workspace's tests
test:
    cargo test --workspace

# everything CI checks: workspace and bench crate (fmt, clippy, tests), firmware (fmt, clippy x2, RAM)
check: && fw-check fw-ram
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    cd {{host}} && cargo fmt -- --check
    cd {{host}} && cargo clippy --all-targets -- -D warnings
    cd {{host}} && cargo test

# the dead-code check, as CLAUDE.md has it: --workspace, never --all-targets
dead-code:
    RUSTFLAGS="--force-warn dead_code" cargo check --workspace

# ---- helpers --------------------------------------------------------------------------------

# the board's serial port: ESP_PORT, or the one usbmodem that is not the CANable
_esp-port:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -n "${ESP_PORT:-}" ]; then echo "$ESP_PORT"; exit 0; fi
    port="$(/bin/ls /dev/cu.usbmodem* 2>/dev/null | grep -v 206E37A | head -1 || true)"
    if [ -z "$port" ]; then
        echo "no board on USB: plug it in (USB before external power); a board that comes and goes: hold BOOT, tap RESET" >&2
        exit 1
    fi
    echo "$port"
