#!/usr/bin/env bash
# Gate 4 — single-catalog.
#
# Every LLM-facing tool name is produced by the one generator in
# `lakelands-api` (GENERATOR below), so the mock, the store's catalog and
# the transports can never disagree on a name. Red when a string literal
# in any `src/` outside the generator makes a tool name:
#   - a per-type door, `<type>_find`, `<type>_get`, `<type>_upsert` or
#     `<type>_retire`;
#   - a door suffix on its own, `"_find"` and the like, from which a name
#     would be assembled;
#   - a format string that assembles one, `"{t}_find"`, `"{}_upsert"`;
#   - a generic tool name (GENERIC below) on a line that names a tool
#     (the line mentions `name` or `tool`), since the bare words are
#     ordinary strings elsewhere.
# Untracked files are checked too, so a local run sees new work.
# Tests may compare against literal names; they read the generator's
# output, they do not serve it.
. "$(dirname "$0")/lib.sh"

GENERATOR='crates/lakelands-api/src/tools'   # tools.rs or tools/
DOORS='find|get|upsert|retire'
GENERIC='describe|query|search|propose|proposals|accept|reject|declare_type|add_field|migrate'

files=0
while IFS= read -r -d '' file; do
    case "$file" in "$GENERATOR".rs|"$GENERATOR"/*) continue ;; esac
    files=$((files + 1))
    while IFS= read -r hit; do fail "$hit"; done < <(perl -ne '
        BEGIN { ($doors, $generic, $file) = splice @ARGV, 0, 3 }
        while (/"([a-z][a-z0-9_]*_(?:$doors))"/g) {
            print "$file:$.: tool name \"$1\" outside the generator\n";
        }
        while (/"(_(?:$doors))"/g) {
            print "$file:$.: tool-name suffix \"$1\" outside the generator\n";
        }
        while (/"([^"\n]*\{[^}"\n]*\}_(?:$doors)\b[^"\n]*)"/g) {
            print "$file:$.: tool name assembled by \"$1\" outside the generator\n";
        }
        if (/\b(?:name|tool)/i) {
            while (/"($generic)"/g) {
                print "$file:$.: tool name \"$1\" outside the generator\n";
            }
        }
    ' "$DOORS" "$GENERIC" "$file" "$file")
done < <(git ls-files -z --cached --others --exclude-standard -- ':(glob)**/src/**/*.rs')

verdict "$files source file(s) outside $GENERATOR"
