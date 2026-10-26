#!/usr/bin/env bash
# Runs the conformance suite against one engine, or against every
# required engine of conformance/roster when none is named. An engine
# whose entry point runs the suite fails on zero tests; only an empty
# engine, whose entry point does not call the runner yet (`run_all(` or `run_checks(`
# outside a `//` comment), passes on none. An engine with
# tests/engine_seam.rs runs the engine-seam harness in the same job.
# Usage: scripts/conformance.sh [engine]      (--list prints the roster)
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

required() { awk '!/^[[:space:]]*(#|$)/ && $2 == "required" { print $1 }' conformance/roster; }

# Whether an engine's tests need a server the run hands them. Only pg's
# do: its harness reads LAKELANDS_TEST_PG_URL.
needs_server() { [ "$1" = pg ]; }

# Whether an entry point calls the suite's runner: `run_all(` or `run_checks(` outside a
# `//` comment. A mention in a comment is not a call. grep reads to the
# end (no -q): stopping at the first match would cut sed off, and under
# pipefail its broken pipe would fail the pipeline.
runs_suite() { sed 's|//.*||' "$1" | grep -E 'run_(all|checks)[[:space:]]*\(' >/dev/null; }

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
    if needs_server "$engine" && [ -z "${LAKELANDS_TEST_PG_URL:-}" ]; then
        echo "conformance: pg needs LAKELANDS_TEST_PG_URL, the PostgreSQL to run against" >&2
        exit 1
    fi
    flags=()
    runs_suite "$crate/tests/conformance.rs" || flags=(--no-tests=pass)
    # The second harness, over the engine traits, runs here too for an
    # engine that has it, so both are in the conformance job.
    tests=(--test conformance)
    [ -f "$crate/tests/engine_seam.rs" ] && tests+=(--test engine_seam)
    # An engine that needs a server has no other place its tests run: the
    # workspace job has no server, and there they stand down. So every
    # test file of its crate runs here, by that rule and not by a list, and
    # a new test file cannot stand down unseen.
    if needs_server "$engine"; then
        tests=()
        for file in "$crate"/tests/*.rs; do
            tests+=(--test "$(basename "$file" .rs)")
        done
    fi
    echo "conformance: $engine runs ${tests[*]}"
    cargo nextest run --locked ${flags[@]+"${flags[@]}"} \
        -p "lakelands-engine-$engine" "${tests[@]}"
    # The mirror's, `verify`'s and the two-process report checks run on
    # every engine the CLI carries; the workspace job has no server, so
    # their PostgreSQL rows run here.
    if needs_server "$engine"; then
        echo "conformance: $engine runs the CLI's mirror, verify and report checks"
        cargo nextest run --locked -p lakelands-cli --test mirror --test verify \
            --test report_across_processes
        # The door checks through a forwarding handle beside a writer on
        # this engine (lakelands#1394).
        echo "conformance: $engine runs the door checks through a forwarding handle"
        cargo nextest run --locked -p lakelands-client --test forwarding
        # Last, once every test above has ended: none left its schema
        # behind (lakelands#1011). This holds only where this run owns the
        # database, as CI's job and a worktree's own container running one
        # suite at a time do; another run in it leaves schemas while it
        # runs.
        echo "conformance: $engine finds no test schema left"
        cargo nextest run --locked -p lakelands-engine-pg --test leftovers --run-ignored only
    fi
done
