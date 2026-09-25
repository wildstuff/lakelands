#!/usr/bin/env bash
# Runs the conformance suite against one engine, or against every
# required engine of conformance/roster when none is named. An engine
# whose entry point runs the suite fails on zero tests; only an empty
# engine, whose entry point does not call the runner yet (`run_all(`
# outside a `//` comment), passes on none. An engine with
# tests/engine_seam.rs runs the engine-seam harness in the same job.
# Usage: scripts/conformance.sh [engine]      (--list prints the roster)
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

required() { awk '!/^[[:space:]]*(#|$)/ && $2 == "required" { print $1 }' conformance/roster; }

# Whether an entry point calls the suite's runner: `run_all(` outside a
# `//` comment. A mention in a comment is not a call.
runs_suite() { sed 's|//.*||' "$1" | grep -q 'run_all[[:space:]]*('; }

if [ "${1:-}" = "--list" ]; then
    required
    exit 0
fi

# Written for bash 3.2 and later, the first shell a stranger has: no
# mapfile, and an array that may be empty is expanded as
# ${a[@]+"${a[@]}"}, since bash before 4.4 calls an empty array unset
# under `set -u`.
engines=("$@")
if [ "${#engines[@]}" -eq 0 ]; then
    while IFS= read -r engine; do engines+=("$engine"); done < <(required)
fi
for engine in ${engines[@]+"${engines[@]}"}; do
    echo "conformance: $engine"
    # An engine whose entry point runs the suite must have tests to run.
    # Only an engine whose tests/conformance.rs does not call the runner
    # yet (its crate still holds nothing but its crate doc) passes on zero
    # tests; the conformance-required gate refuses that for any engine
    # with source, so the allowance ends by itself once an engine is
    # imported and cannot be given back silently.
    crate="crates/lakelands-engine-$engine"
    # The pg engine's tests stand down without a server, so that a
    # workspace run on a machine without PostgreSQL stays green; the
    # conformance run for pg must never stand down.
    if [ "$engine" = pg ] && [ -z "${LAKELANDS_TEST_PG_URL:-}" ]; then
        echo "conformance: pg needs LAKELANDS_TEST_PG_URL, the PostgreSQL to run against" >&2
        exit 1
    fi
    flags=()
    runs_suite "$crate/tests/conformance.rs" || flags=(--no-tests=pass)
    # The second harness, over the engine traits, runs here too for an
    # engine that has it, so both are in the conformance job.
    tests=(--test conformance)
    [ -f "$crate/tests/engine_seam.rs" ] && tests+=(--test engine_seam)
    cargo nextest run --locked ${flags[@]+"${flags[@]}"} \
        -p "lakelands-engine-$engine" "${tests[@]}"
done
