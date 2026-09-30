#!/usr/bin/env bash
# Lists the changes a host must make to its own code when it moves from
# a baseline tag to this tree: one plain line each, for the crates a host
# builds against. Under 0.x a release may break a host; it must say how.
# Usage: scripts/release/host-changes.sh [<baseline-tag>]
#   (default: the nearest `v*` tag that is not a pre-release)
#
# The findings are cargo-semver-checks' (checked against the rules of a
# minor release: under 0.x the tool reads 0.5 -> 0.6 as a major step and
# would report nothing), plus one check of its own: a public trait of
# `lakelands-api` that gained a method without a default, which every
# implementer must add. The tool does not report it for the engine traits.
# With nothing found it says so. It exits 0 whatever it finds, and 2 when
# it cannot run.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

CRATES=(lakelands-api lakelands-client)
TRAIT_FILES=(crates/lakelands-api/src/engine.rs crates/lakelands-api/src/catalog.rs
    crates/lakelands-api/src/seam.rs)

baseline="${1:-$(git describe --tags --abbrev=0 --match 'v*' --exclude 'v*-*' HEAD 2>/dev/null || true)}"
[ -n "$baseline" ] || { echo "host-changes: cannot run: no baseline tag" >&2; exit 2; }
git rev-parse -q --verify "$baseline^{commit}" >/dev/null ||
    { echo "host-changes: cannot run: $baseline is not a revision" >&2; exit 2; }
command -v cargo-semver-checks >/dev/null ||
    { echo "host-changes: cannot run: cargo-semver-checks is not installed" >&2; exit 2; }

# One sentence per lint: what a host has to do. A lint not named here
# keeps the tool's own summary, so nothing found is ever dropped.
advice() {
    case "$1" in
    enum_variant_added) echo "an exhaustive \`match\` on it must handle the new variant" ;;
    enum_struct_variant_field_added) echo "a literal or pattern of the variant without \`..\` must name the new field" ;;
    constructible_struct_adds_field) echo "a struct literal without \`..Default::default()\` must set the new field" ;;
    struct_pub_field_missing | struct_missing | enum_missing | function_missing | trait_missing | enum_variant_missing)
        echo "code that names it must stop doing so" ;;
    trait_method_added | trait_method_parameter_count_changed | trait_method_missing)
        echo "an implementation of the trait must follow" ;;
    function_parameter_count_changed | method_parameter_count_changed)
        echo "every call must pass the new arguments" ;;
    enum_no_repr_variant_discriminant_changed)
        echo "only a cast of the variant with \`as\` sees the new number" ;;
    *) echo "" ;;
    esac
}

out="$(mktemp)"
trap 'rm -f "$out"' EXIT
lines=()
for crate in "${CRATES[@]}"; do
    # Without colour: CI sets CARGO_TERM_COLOR=always, and escape codes
    # would hide every finding from the reading below.
    CARGO_TERM_COLOR=never cargo semver-checks check-release -p "$crate" \
        --baseline-rev "$baseline" --release-type minor --color never >"$out" 2>&1 &&
        status=0 || status=$?
    # Escape codes stripped all the same, should a terminal setting win.
    sed -i 's/\x1b\[[0-9;]*[A-Za-z]//g' "$out"
    grep -q 'Summary' "$out" ||
        { cat "$out" >&2; echo "host-changes: cannot run: cargo semver-checks failed for $crate" >&2; exit 2; }
    # The tool's own count of failed checks: the report must read exactly
    # that many lints, or the reading has gone blind, which is never "no
    # change".
    failed="$(sed -n 's/.*Summary .* \([0-9][0-9]*\) major and \([0-9][0-9]*\) minor checks failed.*/\1 \2/p' "$out" |
        awk '{ print $1 + $2 }')"
    # The tool's exit and its count must agree: it exits 0 when no check
    # failed and non-zero otherwise (100 for failed checks), so a non-zero
    # exit with no failed check counted is a run that did not report,
    # never "no change".
    if { [ "$status" -ne 0 ] && [ "${failed:-0}" -eq 0 ]; } ||
        { [ "$status" -eq 0 ] && [ "${failed:-0}" -ne 0 ]; }; then
        cat "$out" >&2
        echo "host-changes: cannot run: cargo semver-checks exited $status for $crate with ${failed:-no} failed check(s) counted" >&2
        exit 2
    fi
    lints=()
    # Fields apart by the unit separator, which is not blank: an empty
    # field stays empty instead of shifting the next one into its place.
    while IFS=$'\x1f' read -r lint summary item; do
        what="$(advice "$lint")"
        lines+=("\`$crate\`: $item: ${what:-$summary} ($lint)")
        [ -n "$lint" ] && lints+=("$lint")
    done < <(awk '
        /^--- failure / {
            lint = $3; sub(/:$/, "", lint)
            summary = $0; sub(/^--- failure [^:]*: /, "", summary); sub(/ ---$/, "", summary)
            next
        }
        /^Failed in:/ { inside = 1; next }
        inside && /^  / {
            item = $0; sub(/^  /, "", item)
            sub(/,? (in file|in|previously in file) \/.*$/, "", item)
            print lint "\037" summary "\037" item
            next
        }
        { inside = 0 }
    ' "$out" | sort -u)
    read="$(printf '%s\n' ${lints[@]+"${lints[@]}"} | sed '/^$/d' | sort -u | wc -l | tr -d ' ')"
    if [ "$read" -ne "${failed:-0}" ]; then
        cat "$out" >&2
        echo "host-changes: cannot run: cargo semver-checks failed ${failed:-0} check(s) for $crate, and $read were read" >&2
        exit 2
    fi
done

# Required trait methods (no default body) present now and not in the
# baseline, by trait and name.
required() {
    awk '
        /^pub trait [A-Za-z]+/ { t = $3; sub(/[:<{].*/, "", t); depth = 0; intrait = 1 }
        intrait {
            if ($0 ~ /^    fn [a-z_0-9]+/ && $0 ~ /;$/) { f = $0; sub(/^    fn /, "", f); sub(/[<(].*/, "", f); print t "::" f }
            else if ($0 ~ /^    fn [a-z_0-9]+/ && $0 !~ /[{;]$/) { pending = $0 }
            else if (pending != "" && $0 ~ /^    \) -> .*;$|^    \);$/) { f = pending; sub(/^    fn /, "", f); sub(/[<(].*/, "", f); print t "::" f; pending = "" }
            else if ($0 ~ /^    \) .*\{$|^    \) \{$/) { pending = "" }
            if ($0 ~ /^\}/) { intrait = 0; pending = "" }
        }
    '
}
for f in "${TRAIT_FILES[@]}"; do
    [ -f "$f" ] || continue
    while read -r m; do
        case "${lines[*]:-}" in *"$m"*) continue ;; esac
        lines+=("\`lakelands-api\`: trait method $m: an implementation of the trait must add it (trait_method_added)")
    done < <(comm -13 <(git show "$baseline:$f" 2>/dev/null | required | sort -u) <(required < "$f" | sort -u))
done

echo "Changes a host must make, since $baseline:"
echo
if [ "${#lines[@]}" -eq 0 ]; then
    echo "- No change a host must make."
else
    printf '%s\n' "${lines[@]}" | sort -u | sed 's/^/- /'
fi
