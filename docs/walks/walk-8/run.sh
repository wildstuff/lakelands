#!/usr/bin/env bash
# One run of walk 8: a fresh lake from walk 8's setup in a new scratch
# directory "$1" outside the repository, the analyst's tool list, and
# the blind agent on the prompt; writes run-"$2".{jsonl,prompt.txt,txt}
# and tools-"$2".json here. "$3" is the `lakelands` binary.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
scratch=$1 run=$2 bin=$3
"$here/setup.sh" "$scratch/lake" "$bin" > "$scratch/setup.log"
mkdir -p "$scratch/agent"
root="$scratch/lake/walk"
python3 "$here/../walk-7/tools.py" "$bin" --root "$root" --as analyst serve --mcp \
  | sed "s#$scratch/lake#<walk>#g" > "$here/tools-$run.json"
cat > "$scratch/agent/mcp.json" <<JSON
{"mcpServers": {"lake": {"command": "$bin",
  "args": ["--root", "$root", "--as", "analyst", "serve", "--mcp"]}}}
JSON
cp "$here/prompt.txt" "$here/run-$run.prompt.txt"
cd "$scratch/agent"
claude -p "$(cat "$here/run-$run.prompt.txt")" --model claude-opus-5-5 \
  --tools "" --setting-sources project \
  --strict-mcp-config --mcp-config mcp.json \
  --allowedTools mcp__lake \
  --no-session-persistence --output-format stream-json --verbose < /dev/null \
  > "$scratch/raw.jsonl"
python3 "$here/../walk-7/strip.py" "$scratch/raw.jsonl" "$scratch/lake" > "$here/run-$run.jsonl"
python3 "$here/../walk-5/render.py" "$here/run-$run.jsonl" > "$here/run-$run.txt"
