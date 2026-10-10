#!/usr/bin/env bash
# Gate 4 — single-catalog.
#
# Every LLM-facing tool name is produced by the one generator in
# `lakelands-api` (GENERATOR below), so the store's catalog and
# the transports can never disagree on a name. Red when a string literal
# in any `src/` outside the generator makes a tool name:
#   - a per-type door, `<type>_find`, `<type>_get`, `<type>_upsert` or
#     `<type>_retire`, or a link door, `<type>_<relation>_link` or
#     `<type>_<relation>_unlink`;
#   - a door suffix on its own, `"_find"` and the like, from which a name
#     would be assembled;
#   - a format string that assembles one, `"{t}_find"`, `"{}_upsert"`;
#   - a generic tool name on a line that names a tool (the line mentions
#     `name` or `tool` anywhere, inside an identifier such as
#     `describe_tool` too), since the bare words are ordinary strings
#     elsewhere. The generic names are the generator's own, read from its
#     `GENERIC_TOOLS` and the constants it lists: the gate keeps no list,
#     so a tool added there is known here.
# Untracked files are checked too, so a local run sees new work.
# Tests may compare against literal names; they read the generator's
# output, they do not serve it.
. "$(dirname "$0")/lib.sh"

GENERATOR='crates/lakelands-api/src/tools'   # tools.rs or tools/
DOORS='find|get|upsert|retire|link|unlink'

# The generic tool names, joined by `|`: the value of every constant
# `GENERIC_TOOLS` lists. A constant it lists that is not declared as one
# name, or a list whose length is not the one it declares, cannot be read.
GENERIC="$(perl -0777 -ne '
    my %value = /^const ([A-Z][A-Z0-9_]*): &str = "([a-z][a-z0-9_]*)";$/mg;
    my ($n, $list) = /^pub const GENERIC_TOOLS: \[&str; (\d+)\] = \[(.*?)\];$/ms
        or die "no GENERIC_TOOLS\n";
    my @names = $list =~ /([A-Z][A-Z0-9_]*)/g;
    @names == $n or die "GENERIC_TOOLS declares $n names and lists " . @names . "\n";
    defined $value{$_} or die "GENERIC_TOOLS lists $_, which is no constant of one name\n" for @names;
    print join "|", map { $value{$_} } @names;
' "$GENERATOR.rs" 2>&1)" || die "$GENERATOR.rs: $GENERIC"

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
        if (/(?:name|tool)/i) {
            while (/"($generic)"/g) {
                print "$file:$.: tool name \"$1\" outside the generator\n";
            }
        }
    ' "$DOORS" "$GENERIC" "$file" "$file")
done < <(git ls-files -z --cached --others --exclude-standard -- ':(glob)**/src/**/*.rs')

verdict "$files source file(s) outside $GENERATOR, $(tr -cd '|' <<<"$GENERIC|" | wc -c | tr -d ' ') generic name(s)"
