#!/usr/bin/env bash
# Holds the judgement of scripts/release/surface-held.sh to a fixture
# (scripts/release/fixtures/surface-held/): the tool's report of two
# crates, the list and a release's tree, read as text and never built.
# It needs no tag and no cargo-semver-checks.
#
# The fixture's rows, let through: a variant added, a tuple variant's
# field added, a variant's number moved in the enum that gained it, its
# place in a derived order moved there (a warning, read as a failure), a door
# the release marked `#[deprecated]` over several lines and then removed,
# a door the list names, an enum the list names as hidden, a function's signature the list names (from
# host-changes.sh's own checks), and a constant the list names by its
# file. Refused: a variant's number moved in an enum that gained none, its
# place in a derived order moved there, a
# door removed with no deprecation, the listed door's parameters changed
# (the list names its removal, not this lint), an enum the list names as
# hidden turned non-exhaustive too (the list names the one lint), a constant of the same
# name in another file, a field's type changed in place, and a line of
# the list that no change answers. The judgement exits 1.
# Usage: scripts/release/surface-held-test.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

dir=scripts/release/fixtures/surface-held
status=0
got="$(scripts/release/surface-held.sh --judge "$dir/report.txt" "$dir/list.txt" "$dir/tree")" || status=$?
if [ "$status" -ne 1 ]; then
    printf '%s\n' "$got" >&2
    echo "surface-held-test: the judgement exited $status, not 1" >&2
    exit 1
fi
if diff <(printf '%s\n' "$got") "$dir/expected.txt"; then
    echo "surface-held-test: the judgement is the fixture's ($(wc -l < "$dir/expected.txt" | tr -d ' ') lines)"
else
    echo "surface-held-test: the judgement differs from $dir/expected.txt (<: judged, >: expected)" >&2
    exit 1
fi
