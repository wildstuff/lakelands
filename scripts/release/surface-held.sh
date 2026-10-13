#!/usr/bin/env bash
# Holds the host surface to its rule (docs/decisions/056-the-host-surface-
# changes-by-one-release-of-deprecation.md): against the release named in
# scripts/release/surface-accepted, every change `cargo semver-checks`
# finds in the host crates is refused, unless
# - it grows the surface: a variant or a variant's field added, a field
#   added to a struct a host may build, a variant's number moved where
#   the same enum gained a variant; no deprecation can come before an
#   addition;
# - it removes an item the release already marked `#[deprecated]`;
# - or it is a line of the list (the crate, the lint and the item), a
#   change the release's notes name.
# The changes `scripts/release/host-changes.sh` finds by its own checks,
# which the tool does not make (a type changed in place), are judged the
# same way; none of them is an addition or a removal.
# A line of the list that no change answers is refused as well, so the
# list is exactly what the notes say. An item `#[doc(hidden)]` is no
# host's; the tool does not look at it.
# Usage: scripts/release/surface-held.sh
#        scripts/release/surface-held.sh --write
#   (writes the list anew from what is found: each change that is neither
#   an addition nor a deprecated removal, under the same release)
#        scripts/release/surface-held.sh --judge <findings> <list> <dir>
#   (the judgement alone, over the tool's report in <findings>, the list
#   in <list> and a release's tree in <dir>: what surface-held-test.sh
#   holds it by, with no tool and no tag)
# Exits 0 when the surface holds, 1 when a change is refused, 2 when it
# cannot run.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

CRATES=(lakelands-api lakelands-client lakelands-core lakelands-store
    lakelands-engine-mem lakelands-engine-sqlite lakelands-engine-pg)
LIST=scripts/release/surface-accepted
GROWS='^(enum_variant_added|enum_struct_variant_field_added|enum_tuple_variant_field_added|constructible_struct_adds_field)$'
# A variant's number moved: an addition only where the same enum gained a
# variant in the same run; a reorder alone is a change.
MOVED=enum_no_repr_variant_discriminant_changed

mode=check
case "${1:-}" in
--write) mode=write ;;
--judge)
    mode=judge
    [ -f "${2:-}" ] && [ -f "${3:-}" ] && [ -d "${4:-}" ] ||
        { echo "surface-held: cannot run: --judge takes a report, a list and a directory" >&2; exit 2; }
    report="$2" LIST="$3" tree="$4"
    ;;
"") ;;
*) echo "surface-held: cannot run: unknown argument $1" >&2; exit 2 ;;
esac

baseline="$(awk '$1 == "baseline" { print $2; exit }' "$LIST")"
[ -n "$baseline" ] || { echo "surface-held: cannot run: $LIST names no baseline" >&2; exit 2; }

# A file of the baseline's tree, by its path from the repository root.
was() {
    if [ "$mode" = judge ]; then cat "$tree/$1" 2>/dev/null; else git show "$baseline:$1" 2>/dev/null; fi
}

# Whether the item declared at line `$2` of the baseline's file `$1` is
# `#[deprecated]`: an attribute among the doc comments and attributes
# that run into its line, above or from it.
deprecated() {
    was "$1" | awk -v at="$2" '
        { text[NR] = $0 }
        END {
            for (i = at - 1; i >= 1 && text[i] ~ /^[ \t]*(#\[|\/\/\/|\/\/!|\]|\)|[a-z_]+ *=)/; i--)
                if (text[i] ~ /#\[deprecated/) exit 0
            for (i = at; i <= NR && text[i] ~ /^[ \t]*(#\[|\/\/\/|\]|\)|[a-z_]+ *=)/; i++)
                if (text[i] ~ /#\[deprecated/) exit 0
            exit 1
        }'
}

# The tool's report of one crate as lines `crate`, lint, item, and where
# the baseline held it (`crates/...:line`, empty unless the item is gone),
# apart by tabs. A constant the tool names without a path is named with
# its file (`NAME in crates/.../lib.rs`), so two of one name stay apart.
findings() {
    awk -v crate="$1" '
        /^--- failure / { lint = $3; sub(/:$/, "", lint); next }
        /^Failed in:/ { inside = 1; next }
        inside && /^  / {
            item = $0; sub(/^  /, "", item); at = ""; file = ""
            if (match(item, /,? previously in file \/.*$/)) {
                at = substr(item, RSTART); sub(/^,? previously in file /, "", at)
                sub(/^.*\/crates\//, "crates/", at)
            }
            if (match(item, /,? (in file|in|previously in file) \/.*$/)) {
                file = substr(item, RSTART); sub(/^.*\/crates\//, "crates/", file); sub(/:[0-9]+$/, "", file)
            }
            sub(/,? (in file|in|previously in file) \/.*$/, "", item)
            if (lint ~ /const/ && item !~ /::/ && file != "") item = item " in " file
            print crate "\t" lint "\t" item "\t" at
            next
        }
        { inside = 0 }
    ' | LC_ALL=C sort -u
}

all="$(mktemp)"
out="$(mktemp)"
trap 'rm -f "$all" "$out"' EXIT
if [ "$mode" = judge ]; then
    while read -r crate; do
        awk -v c="$crate" '$0 == "=== " c { on = 1; next } /^=== / { on = 0 } on' "$report" | findings "$crate"
    done < <(sed -n 's/^=== //p' "$report" | grep -vx own) >"$all"
    awk '$0 == "=== own" { on = 1; next } /^=== / { on = 0 } on { print $0 "\t" }' "$report" >>"$all"
else
    command -v cargo-semver-checks >/dev/null ||
        { echo "surface-held: cannot run: cargo-semver-checks is not installed" >&2; exit 2; }
    git rev-parse -q --verify "$baseline^{commit}" >/dev/null ||
        { echo "surface-held: cannot run: $baseline is not a revision" >&2; exit 2; }
    for crate in "${CRATES[@]}"; do
        CARGO_TERM_COLOR=never cargo semver-checks check-release -p "$crate" \
            --baseline-rev "$baseline" --release-type minor --color never >"$out" 2>&1 || true
        sed -i 's/\x1b\[[0-9;]*[A-Za-z]//g' "$out"
        grep -q 'Summary' "$out" ||
            { cat "$out" >&2; echo "surface-held: cannot run: cargo semver-checks failed for $crate" >&2; exit 2; }
        findings "$crate" <"$out" >>"$all"
    done
    scripts/release/host-changes.sh --own "$baseline" | sed 's/$/\t/' >>"$all" ||
        { echo "surface-held: cannot run: host-changes.sh --own failed" >&2; exit 2; }
fi

grown=0 gone=0 named=0
refused=()
kept=()
used="$(mktemp)"
trap 'rm -f "$all" "$out" "$used"' EXIT
# The enums that gained a variant, as `crate<tab>Enum`.
gained="$(awk -F '\t' '$2 == "enum_variant_added" { e = $3; sub(/^variant /, "", e); sub(/:.*/, "", e); print $1 "\t" e }' "$all")"
while IFS=$'\t' read -r crate lint item at; do
    if [[ "$lint" =~ $GROWS ]]; then grown=$((grown + 1)); continue; fi
    if [ "$lint" = "$MOVED" ]; then
        e="${item#variant }"; e="${e%%::*}"
        if printf '%s\n' "$gained" | grep -Fxq "$crate"$'\t'"$e"; then grown=$((grown + 1)); continue; fi
    fi
    if [ -n "$at" ] && deprecated "${at%:*}" "${at##*:}"; then gone=$((gone + 1)); continue; fi
    line="$crate"$'\t'"$lint"$'\t'"$item"
    kept+=("$line")
    if grep -Fxq "$line" "$LIST"; then
        named=$((named + 1)); printf '%s\n' "$line" >>"$used"; continue
    fi
    refused+=("\`$crate\`: $item ($lint): neither an addition, nor a removal $baseline deprecated, nor a line of the list")
done <"$all"

if [ "$mode" = write ]; then
    {
        sed -n '/^#/p' "$LIST"
        echo "baseline $baseline"
        [ "${#kept[@]}" -eq 0 ] || printf '%s\n' "${kept[@]}" | LC_ALL=C sort -u
    } >"$out"
    cp "$out" "$LIST"
    echo "surface-held: wrote $LIST: $(printf '%s\n' "${kept[@]}" | sed '/^$/d' | LC_ALL=C sort -u | wc -l | tr -d ' ') line(s) against $baseline"
    exit 0
fi

while IFS= read -r line; do
    grep -Fxq "$line" "$used" ||
        refused+=("\`${line%%$'\t'*}\`: $(printf '%s' "${line#*$'\t'}" | awk -F '\t' '{ print $2 " (" $1 ")" }'): a line of $LIST that no change answers")
done < <(grep -v '^#' "$LIST" | grep -v '^baseline ' | sed '/^$/d')

echo "The host surface against $baseline: $grown addition(s), $gone deprecated removal(s), $named change(s) the list names."
if [ "${#refused[@]}" -gt 0 ]; then
    echo
    echo "Refused:"
    printf '%s\n' "${refused[@]}" | LC_ALL=C sort -u | sed 's/^/- /'
    exit 1
fi
