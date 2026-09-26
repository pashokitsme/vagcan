#!/usr/bin/env bash
# The board's RAM against its budget: builds the `dash` image twice — with BLE (the
# default) and without (`--no-default-features`) — prints each one's RAM sections and
# its 15 biggest statics, and exits non-zero when one is over budget.
#
#   crates/dash/vag-dash-fw/ram-budget.sh
#
# CI runs it in the `firmware` job; run it before and after a firmware change and
# compare (CLAUDE.md, "Firmware RAM").
#
# What is measured. The ESP32-C3's data RAM is one fixed region, and the linker lays
# it out as: `.rwdata_dummy` (the DRAM shadow of the code that runs from IRAM — the two
# buses see the same SRAM), `.data`, `.bss`, `.noinit`, and last `.stack`, which is
# whatever is left up to the region's end. So every byte of statics or of IRAM code is
# a byte off the stack. Two limits per build:
#
#   static  .data* + .bss* + .noinit   must stay at or under its ceiling
#   stack   .stack                     must stay at or over its floor
#
# The stack floor is the one that catches everything, IRAM code included; the static
# ceiling names the usual culprit.
#
# The image is built with `VAGCAN_DASH_NO_CAR=1` — an empty plan, as CI builds it, so
# the numbers are the code's and not one car's (the reference car's plan adds ~1.7 KB
# of statics). Where it is built: see `CARGO_TARGET_DIR` below.
set -euo pipefail

FW="$(cd "$(dirname "$0")" && pwd)"
cd "$FW"

# The budget, in bytes — the one place its numbers live. Measured 2026-09-26 on the
# empty-plan image; each limit leaves ~11–12 % headroom over what it was then:
#
#   build   static   ceiling   stack    floor
#   ble     138,848  156,000   157,116  140,000
#   no-ble  129,388  145,000   188,464  168,000
#
# Crossing one is not forbidden, it is a decision: raise the number here, in the same
# commit, with the reason beside it (CLAUDE.md, "Firmware RAM").
BUILDS=(
	"ble||156000|140000"
	"no-ble|--no-default-features|145000|168000"
)

export VAGCAN_DASH_NO_CAR=1

# Where the images are built.
# - Locally: a directory of its own, `target/ram-budget`, so the empty-plan `dash` this
#   leaves never replaces a flashable one where `cargo run` or `bench.sh` flash from.
# - In CI (`CI` set): the default `target`, beside the clippy steps' host build-deps,
#   so a cold run builds them once and the cache holds one tree. The job never flashes,
#   so an empty-plan image there is never mistaken for one to put on a board.
# - A `CARGO_TARGET_DIR` the caller set is used as it is.
if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
	if [[ -n "${CI:-}" ]]; then
		CARGO_TARGET_DIR="$FW/target"
	else
		CARGO_TARGET_DIR="$FW/target/ram-budget"
	fi
fi
export CARGO_TARGET_DIR
ELF="$CARGO_TARGET_DIR/riscv32imc-unknown-none-elf/release/dash"

# llvm-size and llvm-nm from rustup's `llvm-tools` component, for this directory's
# toolchain (rust-toolchain.toml) — no cargo-binutils.
host="$(rustc -vV | sed -n 's/^host: //p')"
tools="$(rustc --print sysroot)/lib/rustlib/$host/bin"
if [[ ! -x "$tools/llvm-size" || ! -x "$tools/llvm-nm" ]]; then
	echo "adding rustup's llvm-tools component (llvm-size, llvm-nm)"
	rustup component add llvm-tools
fi

# The summed size of the sections in `$sections` whose name matches the regex `$1`.
# Through the environment, not `awk -v`, which would eat the backslashes.
size_of() { RE="$1" awk '$1 ~ ENVIRON["RE"] { s += $2 } END { print s + 0 }' <<<"$sections"; }

over=0
for build in "${BUILDS[@]}"; do
	IFS='|' read -r name flags ceiling floor <<<"$build"
	echo "== dash, $name =="
	# `$flags` is unquoted on purpose: empty is no argument at all.
	# shellcheck disable=SC2086
	cargo build --release --bin dash $flags --quiet
	[[ -f "$ELF" ]] || {
		echo "no image at $ELF"
		exit 1
	}

	# `llvm-size -A`: one `name size addr` row per section.
	sections="$("$tools/llvm-size" -A "$ELF")"
	data=$(size_of '^\.data')
	bss=$(size_of '^\.bss')
	noinit=$(size_of '^\.noinit$')
	iram=$(size_of '^\.rwdata_dummy$')
	stack=$(size_of '^\.stack$')
	static=$((data + bss + noinit))

	printf '  %-8s %8d\n' .data "$data" .bss "$bss" .noinit "$noinit" "iram" "$iram" .stack "$stack"
	echo "  biggest statics:"
	# `address size type name`, sizes in decimal; b/d are .bss/.data, lower case local.
	"$tools/llvm-nm" --size-sort --reverse-sort --print-size --radix=d --demangle "$ELF" |
		awk '$3 ~ /^[bBdD]$/ && shown++ < 15 { size = $2 + 0; sub(/^[^ ]+ +[^ ]+ +[^ ]+ +/, ""); printf "  %8d  %s\n", size, $0 }'

	if ((static > ceiling)); then
		echo "OVER: static $static B, ceiling $ceiling B — $((static - ceiling)) B over"
		over=1
	else
		echo "ok: static $static B, ceiling $ceiling B — $((ceiling - static)) B left"
	fi
	if ((stack < floor)); then
		echo "OVER: stack $stack B, floor $floor B — $((floor - stack)) B short"
		over=1
	else
		echo "ok: stack $stack B, floor $floor B — $((stack - floor)) B to spare"
	fi
done

if ((over)); then
	echo "RAM over budget. Shrink it, or raise the limit in ram-budget.sh with the reason (CLAUDE.md, \"Firmware RAM\")."
	exit 1
fi
