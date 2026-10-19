#!/bin/sh
# Every channel the car's dash build can resolve → <out>/catalog.json and catalog-summary.txt.
#
#   research/dash/layout-editor/catalog/run.sh <VIN> <out-dir> [survey.jsonl ... recording.csv ...]
#
# Surveys (`research/dumps/survey-*.jsonl`, untracked) mark which identifiers answered;
# recordings (`research/captures/*.csv`) add the columns they read. Without them `answered` is null.
# Read-only on ~/.vagcan: the tool runs with HOME pointing at a fresh copy of it.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
VIN=${1:?usage: run.sh <VIN> <out-dir> [evidence files...]}
OUT=${2:?usage: run.sh <VIN> <out-dir> [evidence files...]}
shift 2
TARGET=${CARGO_TARGET_DIR:-$HERE/../target}
COPY=$(mktemp -d)
trap 'rm -rf "$COPY"' EXIT
rsync -a --exclude rod ~/.vagcan/ "$COPY/.vagcan/"
(cd "$HERE" && CARGO_TARGET_DIR="$TARGET" cargo build --release -q)
# The identifier pages the August surveys asked (.archive/research/car/whole-car-survey.md).
PAGES=0200-02FF,0600-06FF,1900-19FF,2000-22FF,2A00-2BFF,3800-38FF,F100-F1FF,F400-F4FF
mkdir -p "$OUT"
HOME="$COPY" "$TARGET/release/dash-catalog" "$VIN" "$OUT" "$PAGES" "$@"
