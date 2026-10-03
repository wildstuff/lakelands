#!/usr/bin/env bash
# Holds the three comparisons of scripts/release/host-changes.sh to a
# fixture: two small files, read as text and never built, and the report
# expected of them (scripts/release/fixtures/host-changes/). It needs no
# baseline tag and no cargo-semver-checks.
#
# The fixture's rows: a type written by a shorter path, in a trait
# method, a function, a method and a field, which is not reported; a path
# that becomes another path of the same last name, and a type that
# changes, which are; a renamed parameter and a private field, which are
# not; and `Self::Out` becoming `Out`, which the rule reads as one and
# the script's head names as a miss.
# Usage: scripts/release/host-changes-test.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

dir=scripts/release/fixtures/host-changes
got="$(scripts/release/host-changes.sh --compare "$dir/was.rs" "$dir/now.rs")"
if diff <(printf '%s\n' "$got") "$dir/expected.txt"; then
    echo "host-changes-test: the report is the fixture's ($(wc -l < "$dir/expected.txt" | tr -d ' ') lines)"
else
    echo "host-changes-test: the report differs from $dir/expected.txt (<: reported, >: expected)" >&2
    exit 1
fi
