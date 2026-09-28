#!/usr/bin/env bash
# Gate 3 — vocabulary.
#
# Lakelands works below the platform it was extracted from and does not
# know that platform exists. Its tenant is a lake, and none of the
# platform's own words survive an import. Red when
#   - a file under any `src/` directory, or any `.rs` file under
#     `crates/` (tests, benches and examples included), contains one of
#     SRC_WORDS as an identifier fragment or a word, case-insensitively
#     (so `wild_data`, `WildData`, `WILD_` and "the wild" are all
#     refused);
#   - any tracked file, any tracked path, or any commit message on the
#     pull request contains one of REPO_WORDS, case-insensitively.
# This script is the only file that may spell the lists out.
. "$(dirname "$0")/lib.sh"

SRC_WORDS='tribe|root_tribe|wild_|wild|chief|elder|atlas'
REPO_WORDS='atlas'

scan() { # <regex> <pathspec...>
    local re="$1"; shift
    while IFS= read -r -d '' file; do
        [ "$file" = "$GATE_SELF" ] && continue
        grep -I -n -o -i -E -- "$re" "$file" 2>/dev/null \
            | while IFS= read -r hit; do printf '%s:%s\n' "$file" "$hit"; done || true
    done < <(git ls-files -z -- "$@")
}

while IFS= read -r hit; do
    fail "$hit (refused in code)"
done < <(scan "$SRC_WORDS" ':(glob)**/src/**' ':(glob)crates/**/*.rs')

while IFS= read -r hit; do
    fail "$hit (refused repository-wide)"
done < <(scan "$REPO_WORDS" .)

while IFS= read -r path; do
    fail "path $path (refused repository-wide)"
done < <(git ls-files | grep -i -E -- "$REPO_WORDS" || true)

range="$(gate_range)"
while IFS= read -r commit; do
    # grep reads to the end (no -q), so git is never cut off mid-write.
    if git log -1 --format='%B' "$commit" | grep -i -E -- "$REPO_WORDS" >/dev/null; then
        fail "commit $(git log -1 --format='%h %s' "$commit"): message contains a refused word"
    fi
done < <(git rev-list "$range")

verdict "range $range"
