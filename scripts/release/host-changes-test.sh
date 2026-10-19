#!/usr/bin/env bash
# Holds the three comparisons of scripts/release/host-changes.sh to a
# fixture: two small files, read as text and never built, and the report
# expected of them (scripts/release/fixtures/host-changes/). It needs no
# baseline tag and no cargo-semver-checks.
#
# The fixture's rows: a type written by a shorter path, in a trait
# method, a function, a method and a field, which is not reported; a path
# that becomes another path of the same last name, and a type that
# changes, which are, in a struct and a variant each written on one line
# too; a renamed parameter and a private field, which are not; and
# `Self::Out` becoming `Out`, which the rule reads as one and the
# script's head names as a miss. Below them, a trait, a function, a
# struct and a method under `#[doc(hidden)]` whose types change, which
# are not reported: they are no host's.
#
# Then `reach` over a small tree (tree/src): an item of a hidden `pub mod`
# that a visible `pub use` re-exports reaches a host by the parent's path,
# one only a hidden re-export names or none names does not, an item of a
# public module does, and one of a hidden module that re-exports nothing
# does not. The item that bears its module's name is among them.
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
reached="$(while read -r file item; do
    printf '%s %s: %s\n' "$file" "$item" "$(scripts/release/host-changes.sh --reach "$dir/tree/src/$file" "$item")"
done < <(sed 's/: .*//' "$dir/reach-expected.txt"))"
if diff <(printf '%s\n' "$reached") "$dir/reach-expected.txt"; then
    echo "host-changes-test: reach is the fixture's ($(wc -l < "$dir/reach-expected.txt" | tr -d ' ') items)"
else
    echo "host-changes-test: reach differs from $dir/reach-expected.txt (<: reached, >: expected)" >&2
    exit 1
fi
