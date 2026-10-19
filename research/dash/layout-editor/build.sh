#!/bin/sh
# Assemble the layout editor into out/: the page, the dash renderer as WebAssembly, and the
# car's channel list. Then serve it: python3 -m http.server -d research/dash/layout-editor/out
#
#   research/dash/layout-editor/build.sh <VIN> [survey.jsonl ... recording.csv ...]
#
# Needs: rustup target add wasm32-unknown-unknown; `vagcan setup` done for the car's project.
# Read-only on ~/.vagcan: the channel list is made against a copy of it (catalog/run.sh).
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
VIN=${1:?usage: build.sh <VIN> [survey.jsonl ... recording.csv ...]}
shift
OUT=$HERE/out
TARGET=${CARGO_TARGET_DIR:-$HERE/target}
mkdir -p "$OUT"
(cd "$HERE/wasm" && CARGO_TARGET_DIR="$TARGET" cargo build --release --target wasm32-unknown-unknown -q)
cp "$TARGET/wasm32-unknown-unknown/release/dash_wasm.wasm" "$OUT/dash.wasm"
"$HERE/catalog/run.sh" "$VIN" "$OUT" "$@"
# The editor needs a slice of the catalog: 4 MB → 1.5 MB.
python3 - "$OUT/catalog.json" "$OUT/channels.json" <<'PY'
import json, sys
keep = ('ref', 'unit', 'unitName', 'name', 'ru', 'did', 'uom', 'decimals', 'answered', 'proven', 'aliases', 'label')
rows = json.load(open(sys.argv[1]))
slim = [{k: r[k] for k in keep if r.get(k) not in (None, [], '')} | {'numeric': r['numeric']} for r in rows]
json.dump(slim, open(sys.argv[2], 'w'), ensure_ascii=False, separators=(',', ':'))
PY
cp "$HERE/index.html" "$OUT/index.html"
echo "built $OUT — serve it: python3 -m http.server -d $OUT"
