#!/usr/bin/env bash
# Runs the conformance suite against one engine, or against every
# required engine of conformance/roster when none is named.
# Usage: scripts/conformance.sh [engine]      (--list prints the roster)
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

required() { awk '!/^[[:space:]]*(#|$)/ && $2 == "required" { print $1 }' conformance/roster; }

if [ "${1:-}" = "--list" ]; then
    required
    exit 0
fi

engines=("$@")
[ "${#engines[@]}" -gt 0 ] || mapfile -t engines < <(required)
for engine in "${engines[@]}"; do
    echo "conformance: $engine"
    # --no-tests=pass until the suite has its first case (the store step).
    cargo nextest run --locked --no-tests=pass \
        -p "lakelands-engine-$engine" --test conformance
done
