#!/usr/bin/env bash
# Gate 10 — doc-heads-its-item.
#
# A doc comment stays on the item it documents. Red when a pull request
# adds an item to a `.rs` file (`fn`, `struct`, `enum`, `const`,
# `static`, `type`, `trait`, `impl` or `mod`) and the doc block above it
# is headed by an unchanged `///` line: the new item was inserted between
# an existing comment and the item it belonged to, so that comment now
# heads the new one and the old item has none. It compiles and every
# test stays green, so nothing else notices.
#
# For each added item line in the pull request's diff (from the merge
# base, so changes on the base branch never count), the gate walks up
# through the added `///` and `#[…]` lines directly above it. If the
# next line up is an unchanged `///`, the item is named. A removed line
# there ends the walk: an item that replaces another under its comment
# is a change to that item, not an insertion.
. "$(dirname "$0")/lib.sh"

range="$(gate_range)"
# The diff of the pull request alone: from the merge base of the range.
diff_range="${range/../...}"
n=0
while IFS= read -r violation; do
    case "$violation" in
        items:*) n="${violation#items:}" ;;
        *) fail "$violation" ;;
    esac
done < <(git diff --no-color --no-ext-diff -U3 "$diff_range" -- '*.rs' | awk '
    function is_item(s) {
        return s ~ /^\+[ \t]*(pub(\([^)]*\))?[ \t]+)?((async|const|unsafe|extern)[ \t]+)*(fn|struct|enum|const|static|type|trait|impl|mod)([ \t<(]|$)/
    }
    /^\+\+\+ / { file = substr($0, 7); n = 0; next }
    /^--- / { next }
    /^@@/ { n = 0; next }
    {
        line[++n] = $0
        if (!is_item($0)) next
        items++
        j = n - 1
        while (j > 0 && line[j] ~ /^\+[ \t]*(\/\/\/|#\[)/) j--
        if (j > 0 && line[j] ~ /^ [ \t]*\/\/\//) {
            item = substr($0, 2); sub(/^[ \t]+/, "", item)
            doc = substr(line[j], 2); sub(/^[ \t]+/, "", doc)
            printf "%s: `%s` was added under the doc comment of another item, which reads \"%s\"\n", file, item, doc
        }
    }
    END { print "items:" items + 0 }
')

verdict "$n item(s) added in $diff_range"
