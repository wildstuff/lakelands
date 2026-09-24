#!/usr/bin/env bash
# Gate 6 — core-size.
#
# The core stays small enough to read, and it only ever gets smaller
# than the ceiling it has reached. The count is every line of every
# tracked `src/**/*.rs` in CRATES; `tests/` directories are not counted.
# Red when
#   - the count exceeds the ratchet base (BASE_FILE);
#   - the ratchet base exceeds CEILING;
#   - the ratchet base is higher than on the pull request's base commit
#     (the base only ever moves down; lower it when the core shrinks).
. "$(dirname "$0")/lib.sh"

CEILING=70000
CRATES=(lakelands-core lakelands-store lakelands-engine-sqlite lakelands-engine-pg)
BASE_FILE=scripts/gates/core-size.base

[ -f "$BASE_FILE" ] || die "$BASE_FILE is missing"
base="$(tr -d '[:space:]' < "$BASE_FILE")"
[[ "$base" =~ ^[0-9]+$ ]] || die "$BASE_FILE does not hold a number"

specs=()
for c in "${CRATES[@]}"; do specs+=(":(glob)crates/$c/src/**/*.rs"); done
count="$(git ls-files -z -- "${specs[@]}" | xargs -0 -r cat | wc -l | tr -d ' ')"

[ "$count" -le "$base" ] || fail "core is $count lines, ratchet base is $base"
[ "$base" -le "$CEILING" ] || fail "ratchet base $base is above the ceiling $CEILING"

range="$(gate_range)"
if [ "${range%..HEAD}" != "$range" ]; then
    prev="$(git show "${range%..HEAD}:$BASE_FILE" 2>/dev/null | tr -d '[:space:]' || true)"
    if [[ "$prev" =~ ^[0-9]+$ ]] && [ "$base" -gt "$prev" ]; then
        fail "ratchet base rose from $prev to $base; it only moves down"
    fi
fi

verdict "$count lines of ${CRATES[*]}; base $base, ceiling $CEILING"
