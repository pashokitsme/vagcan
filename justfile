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

# Flags after the image name go to cargo: `--no-default-features` for a board whose BLE does not
# start, `--features bench` for the bench images (they transmit: never in a car).
_elf bin:
    @echo "${CARGO_TARGET_DIR:-target}/riscv32imc-unknown-none-elf/release/{{bin}}"

# build an image: `just fw-build`, `just fw-build dash --no-default-features`, `just fw-build oledtest`
fw-build bin="dash" *flags:
    cd {{fw}} && cargo build --release --bin {{bin}} {{flags}}

# build and flash an image (oledtest: the test picture; slcan: a plain adapter), same flags
fw-flash bin="dash" *flags: (fw-build bin flags)
    port="$(just _esp-port)" && cd {{fw}} && espflash flash --chip esp32c3 --partition-table partitions.csv --non-interactive \
        --port "$port" "$(just _elf {{bin}})"

# build, flash and stay on the board's console (Ctrl-C leaves), same flags
fw-run bin="dash" *flags: (fw-build bin flags)
    port="$(just _esp-port)" && cd {{fw}} && espflash flash --chip esp32c3 --partition-table partitions.csv --monitor \
        --port "$port" "$(just _elf {{bin}})"

# the board's console, without flashing (opening the port resets the board)
fw-monitor:
    port="$(just _esp-port)" && espflash monitor --chip esp32c3 --port "$port"

# which board is on USB: chip revision (v0.4 has no working BLE), MAC
fw-info:
    port="$(just _esp-port)" && espflash board-info --chip esp32c3 --non-interactive --port "$port" | grep -E "Chip type|MAC|Flash size"

# CI `firmware`: fmt, clippy with BLE, without it and with every feature; empty plan, no car needed
fw-check:
    cd {{fw}} && cargo fmt -- --check
    cd {{fw}} && VAGCAN_DASH_NO_CAR=1 cargo clippy --release --bins -- -D warnings
    cd {{fw}} && VAGCAN_DASH_NO_CAR=1 cargo clippy --release --bins --no-default-features -- -D warnings
    cd {{fw}} && VAGCAN_DASH_NO_CAR=1 cargo clippy --release --bins --all-features -- -D warnings

# CI `firmware`: the static-RAM budget, with and without BLE
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

# install vagcan and vagcan-measure into ~/.cargo/bin, over whatever installed them before
# (an old checkout's `crates/vagcan` package included); extra flags go to cargo: `just install --locked`
install *args:
    cargo install --force --path crates/cli/vag-cli {{args}}
    cargo install --force --path crates/cli/vag-cli-measure {{args}}

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

# ---- checks: each recipe is one CI job, so a green `just check` is a green CI ------------------

# rustfmt everywhere: the workspace, the bench crate and the firmware
fmt:
    cargo fmt --all
    cd {{host}} && cargo fmt
    cd {{fw}} && cargo fmt

# CI `fmt`: the workspace is rustfmt-clean
fmt-check:
    cargo fmt --all -- --check

# CI `clippy`: the workspace, tests and examples included
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# CI `test`: the workspace's tests
test:
    cargo test --workspace

# CI `no-std`: the three crates the board links, without `std`, for the board's target
no-std:
    cargo check -p vag-uds-transport --no-default-features --target riscv32imc-unknown-none-elf
    cargo check -p vag-uds-client --no-default-features --target riscv32imc-unknown-none-elf
    cargo check -p vag-uds-can --no-default-features --target riscv32imc-unknown-none-elf

# CI `bench-host`: the bench crate outside the workspace (fmt, clippy, tests)
host-check:
    cd {{host}} && cargo fmt -- --check
    cd {{host}} && cargo clippy --all-targets -- -D warnings
    cd {{host}} && cargo test

# CI `dead-code`: --workspace, never --all-targets (CLAUDE.md); fails on a dead symbol in crates/
dead-code:
    #!/usr/bin/env bash
    set -euo pipefail
    log="$(RUSTFLAGS='--force-warn dead_code' cargo check --workspace 2>&1 | tee /dev/stderr)"
    # --force-warn lints every dependency too; only a warning pointing into crates/ counts
    if grep -E -A2 'never (used|read|constructed)' <<<"$log" | grep -qE '^\s*-->\s.*crates/'; then
        echo "dead code in a workspace crate: a symbol nobody calls. If it was written to be called, the fix is the missing call site, not deletion (CLAUDE.md)." >&2
        exit 1
    fi

# everything CI checks, in CI's jobs: workspace, no-std, bench crate, firmware, dead code
check: fmt-check clippy test no-std host-check fw-check fw-ram dead-code

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
