#!/usr/bin/env bash
# Makes a fresh walk-8 lake in the empty directory "$1", with the
# `lakelands` binary "$2" (default: the one on PATH).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
bin=${2:-lakelands}
mkdir -p "$1" && cd "$1"
"$bin" init walk
cd walk
"$bin" lake new trade
mkdir trade data && cp -r "$here/lake/types" trade/
python3 "$here/lake/make_data.py" write data
"$bin" apply trade
for f in suppliers products customers orders order_lines; do
  "$bin" ingest "data/$f.ndjson" --source erp
done
python3 - "$here/lake/principal.json" <<'PY'
import json, sys
root = json.load(open("lakelands.json"))
root.setdefault("principals", []).append(json.load(open(sys.argv[1])))
json.dump(root, open("lakelands.json", "w"), indent=2)
PY
