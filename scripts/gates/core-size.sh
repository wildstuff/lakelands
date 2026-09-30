#!/usr/bin/env bash
# Gate 6 — core-size.
#
# The core stays small enough to read, and its specification may grow
# without taking room from it. Two budgets are counted, over every line
# of every tracked `src/**/*.rs` in CRATES (`tests/` directories beside
# `src/` are not counted):
#   - specification: the conformance suite (`src/conformance/**`) and the
#     unit tests (every `tests.rs`, and everything under a `tests/`
#     directory inside `src/`);
#   - product: every other file, doc comments included.
# What is test code is decided by path alone, so a unit test lives in a
# `tests.rs` beside its module, never inline. The contract crate is
# counted with the core, so code moved between them is not read as code
# cut.
# Red when
#   - a budget's count exceeds its ratchet base (its BASE file);
#   - a ratchet base exceeds its CEILING;
#   - a ratchet base is higher than on the pull request's base commit
#     (a base only ever moves down, by a decision recorded on lakelands#20);
#   - a product file holds a test (`#[test]`, `#[tokio::test]`) or a
#     `#[cfg(test)]` anywhere but directly before `mod tests;`.
. "$(dirname "$0")/lib.sh"

CRATES=(lakelands-api lakelands-core lakelands-store lakelands-engine-sqlite lakelands-engine-pg)
PRODUCT_CEILING=36000
PRODUCT_BASE=scripts/gates/core-size.product.base
SPEC_CEILING=40000
SPEC_BASE=scripts/gates/core-size.spec.base

all=()
for c in "${CRATES[@]}"; do all+=(":(glob)crates/$c/src/**/*.rs"); done
spec=()
for c in "${CRATES[@]}"; do
    spec+=(":(glob)crates/$c/src/conformance/**/*.rs"
        ":(glob)crates/$c/src/**/tests.rs"
        ":(glob)crates/$c/src/**/tests/**/*.rs")
done
product=("${all[@]}")
for s in "${spec[@]}"; do product+=(":(exclude,glob)${s#:(glob)}"); done

lines() { git ls-files -z -- "$@" | xargs -0 -r cat | wc -l | tr -d ' '; }

summary=()
# budget NAME COUNT BASE_FILE CEILING
budget() {
    local name="$1" count="$2" file="$3" ceiling="$4" base prev range
    [ -f "$file" ] || die "$file is missing"
    base="$(tr -d '[:space:]' < "$file")"
    [[ "$base" =~ ^[0-9]+$ ]] || die "$file does not hold a number"
    [ "$count" -le "$base" ] || fail "$name is $count lines, its ratchet base is $base"
    [ "$base" -le "$ceiling" ] || fail "$name ratchet base $base is above its ceiling $ceiling"
    range="$(gate_range)"
    if [ "${range%..HEAD}" != "$range" ]; then
        prev="$(git show "${range%..HEAD}:$file" 2>/dev/null | tr -d '[:space:]' || true)"
        if [[ "$prev" =~ ^[0-9]+$ ]] && [ "$base" -gt "$prev" ]; then
            fail "$name ratchet base rose from $prev to $base; it only moves down"
        fi
    fi
    summary+=("$name $count of base $base, ceiling $ceiling")
}

# Test code in a product file would leave the specification's count.
while IFS= read -r line; do
    fail "$line"
done < <(git ls-files -z -- "${product[@]}" | xargs -0 -r awk '
    FNR == 1 { prev = "" }
    /^[[:space:]]*#\[(tokio::)?test/ {
        print FILENAME ":" FNR ": a test in a product file; it belongs in a tests.rs"
    }
    prev ~ /^[[:space:]]*#\[cfg\(test\)\][[:space:]]*$/ && $0 !~ /^[[:space:]]*mod tests;[[:space:]]*$/ {
        print FILENAME ":" FNR ": #[cfg(test)] stands before something other than `mod tests;`"
    }
    { prev = $0 }
')

budget product "$(lines "${product[@]}")" "$PRODUCT_BASE" "$PRODUCT_CEILING"
budget specification "$(lines "${spec[@]}")" "$SPEC_BASE" "$SPEC_CEILING"

verdict "${summary[0]}; ${summary[1]}"
