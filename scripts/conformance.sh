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
    # The suite (lakelands-core::conformance) compiles and is registered
    # from O2 on, but an engine's tests/conformance.rs calls its runner only
    # once a store exists to run it against (O3). Until then the entry
    # points are empty on purpose, and this passes on zero tests.
    cargo nextest run --locked --no-tests=pass \
        -p "lakelands-engine-$engine" --test conformance
done
