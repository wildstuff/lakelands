#!/usr/bin/env bash
# Gate 15 — conflict-markers.
#
# A merge or a rebase that is not resolved leaves its markers in the
# files it could not merge, and nothing else reads them as an error: a
# decision record or a page builds with them. Red when any tracked file
# holds a line that begins with seven `<` or seven `>` and a space, or
# that is seven `=` alone.
. "$(dirname "$0")/lib.sh"

files=0
while IFS= read -r -d '' file; do
    files=$((files + 1))
    while IFS= read -r line; do
        fail "$file:$line (an unresolved conflict)"
    done < <(grep -I -n -E -- '^(<{7} |={7}$|>{7} )' "$file" | cut -d: -f1 || true)
done < <(tracked_files)

verdict "$files tracked files"
