#!/usr/bin/env bash
# Lists the changes a host must make to its own code when it moves from
# a baseline tag to this tree: one plain line each, for the crates a host
# builds against. Under 0.x a release may break a host; it must say how.
# Usage: scripts/release/host-changes.sh [<baseline-tag>]
#   (default: the nearest `v*` tag that is not a pre-release)
#
# The findings are cargo-semver-checks' (checked against the rules of a
# minor release: under 0.x the tool reads 0.5 -> 0.6 as a major step and
# would report nothing), plus two checks of its own over the public
# traits of `lakelands-api`, which the tool does not make for them: a
# method without a default that was added, which every implementer must
# add, and a method whose signature changed while its parameter count did
# not, which every implementer must follow. A method a line already names
# is not reported twice. With nothing found it says so. It exits 0
# whatever it finds, and 2 when it cannot run.
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

# Every method of a public trait, one line each, fields apart by a tab:
# `Trait::method`, `required` or `provided` (a default body), its
# signature as written, and the same without parameter names, which is
# what is compared: a renamed parameter asks nothing of an implementation.
# A signature is read from its `fn` (or `async fn`) at the trait's indent
# to its `;` or the `{` of its body, without comments or attributes, with
# whitespace collapsed and rustfmt's trailing comma of the vertical form
# dropped, so the one-line and the vertical form read the same.
methods() {
    awk '
        # The signature without its parameter names: a `name: ` (or
        # `mut name: `) at the start of a parameter of the outer list.
        function unnamed(s,    out, n, i, c, d, a, start, rest) {
            out = ""; d = 0; a = 0; start = 0; n = length(s)
            for (i = 1; i <= n; i++) {
                c = substr(s, i, 1)
                if (c == "-" && substr(s, i + 1, 1) == ">") { out = out "->"; i++; start = 0; continue }
                if (start && c == " ") { out = out c; continue }
                if (start && d == 1 && a == 0) {
                    rest = substr(s, i)
                    if (match(rest, /^(mut )?[A-Za-z_][A-Za-z0-9_]*: /)) { i += RLENGTH - 1; start = 0; continue }
                }
                start = 0
                if (c == "(") { d++; if (d == 1 && a == 0) start = 1 }
                else if (c == ")") d--
                else if (c == "<") a++
                else if (c == ">") a--
                else if (c == "," && d == 1 && a == 0) start = 1
                out = out c
            }
            return out
        }
        function done(kind) {
            gsub(/[ \t]+/, " ", sig)
            sub(/^ /, "", sig); sub(/ $/, "", sig)
            gsub(/\( /, "(", sig); gsub(/ \)/, ")", sig)
            gsub(/,\)/, ")", sig); gsub(/, >/, ">", sig); gsub(/,>/, ">", sig)
            print t "::" name "\t" kind "\t" sig "\t" unnamed(sig)
            sig = ""; reading = 0
        }
        /^pub trait [A-Za-z]/ { t = $3; sub(/[:<{].*/, "", t); intrait = 1; next }
        !intrait { next }
        /^\}/ { intrait = 0; reading = 0; next }
        !reading && /^    (async )?fn [A-Za-z_]/ {
            name = $0; sub(/^    (async )?fn /, "", name); sub(/[^A-Za-z0-9_].*/, "", name)
            reading = 1; sig = ""
        }
        reading {
            line = $0
            if (line ~ /^[ \t]*(#\[|\/\/)/) next
            sub(/[ \t]*\/\/.*$/, "", line)
            sig = sig " " line
            if (sig ~ /\{/) { sub(/[ \t]*\{.*$/, "", sig); done("provided") }
            else if (sig ~ /;[ \t]*$/) { sub(/;[ \t]*$/, "", sig); done("required") }
        }
    '
}

# Whether a line of the report already names `Trait::method`, and not
# only a longer name it begins (`get` in `get_many`).
named() {
    [ "${#lines[@]}" -gt 0 ] &&
        printf '%s\n' "${lines[@]}" | grep -Eq "(^|[^A-Za-z0-9_:])$1([^A-Za-z0-9_]|$)"
}

for f in "${TRAIT_FILES[@]}"; do
    [ -f "$f" ] || continue
    base="$(git show "$baseline:$f" 2>/dev/null | methods)" || base=""
    head="$(methods < "$f")"
    # Required now and not in the baseline: added, or its default dropped.
    while read -r m; do
        [ -n "$m" ] || continue
        named "$m" && continue
        lines+=("\`lakelands-api\`: trait method $m: an implementation of the trait must add it (trait_method_added)")
    done < <(LC_ALL=C comm -13 \
        <(printf '%s\n' "$base" | awk -F '\t' '$2 == "required" { print $1 }' | LC_ALL=C sort -u) \
        <(printf '%s\n' "$head" | awk -F '\t' '$2 == "required" { print $1 }' | LC_ALL=C sort -u))
    # In both, with another signature once the parameter names are left
    # out. Under `async_trait` an `async` that comes or goes is a change.
    while IFS=$'\t' read -r m was was_unnamed now now_unnamed; do
        [ "$was_unnamed" != "$now_unnamed" ] || continue
        named "$m" && continue
        lines+=("\`lakelands-api\`: trait method $m: an implementation of the trait must follow: was \`$was\`, now \`$now\` (trait_method_signature_changed)")
    done < <(LC_ALL=C join -t $'\t' -o 1.1,1.3,1.4,2.3,2.4 \
        <(printf '%s\n' "$base" | sed '/^$/d' | LC_ALL=C sort -t $'\t' -k1,1) \
        <(printf '%s\n' "$head" | sed '/^$/d' | LC_ALL=C sort -t $'\t' -k1,1))
done

echo "Changes a host must make, since $baseline:"
echo
if [ "${#lines[@]}" -eq 0 ]; then
    echo "- No change a host must make."
else
    printf '%s\n' "${lines[@]}" | sort -u | sed 's/^/- /'
fi
