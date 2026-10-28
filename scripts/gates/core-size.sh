#!/usr/bin/env bash
# Gate 6 — core-size.
#
# The core stays small enough to read, and its specification may grow
# without taking room from it. Two budgets are counted, over every line
# of every tracked `.rs` file under `src/` and under `tests/` of each
# crate in CRATES:
#   - specification: the conformance suite (`src/conformance/**`), the
#     unit tests (every `tests.rs`, and everything under a `tests/`
#     directory inside `src/`) and the integration tests (everything
#     under the `tests/` directory beside `src/`);
#   - product: every other file under `src/`, doc comments included.
# The openCypher subset (`lakelands-cypher`) is no core crate: it is
# counted by the same split under budgets of its own, `cypher product`
# and `cypher specification`, so it takes no room from the core and is
# no unbudgeted place to grow.
# What is test code is decided by path alone, so a unit test lives in a
# `tests.rs` beside its module, never inline. The contract crate is
# counted with the core, so code moved between them is not read as code
# cut; the memory engine is counted as the other engines are.
# Red when
#   - a budget's count exceeds its ratchet base (its BASE file);
#   - a ratchet base exceeds its CEILING;
#   - a ratchet base or a ceiling is higher than on the pull request's
#     base commit, unless the same pull request adds the row to
#     docs/budgets.md that raises it to that number (a budget goes down by
#     itself and is raised only by the rule of
#     docs/decisions/047-a-ceiling-is-raised-by-a-recorded-rule.md); a
#     budget absent on the base commit stood at 0 there;
#   - a product file holds a test (`#[test]`, `#[tokio::test]`) or a
#     `#[cfg(test)]` anywhere but directly before `mod tests;`.
. "$(dirname "$0")/lib.sh"

CRATES=(lakelands-api lakelands-core lakelands-store lakelands-engine-mem lakelands-engine-sqlite lakelands-engine-pg)
PRODUCT_CEILING=94500
PRODUCT_BASE=scripts/gates/core-size.product.base
SPEC_CEILING=174000
SPEC_BASE=scripts/gates/core-size.spec.base
CYPHER_CRATES=(lakelands-cypher)
CYPHER_PRODUCT_CEILING=3500
CYPHER_PRODUCT_BASE=scripts/gates/cypher-size.product.base
CYPHER_SPEC_CEILING=1500
CYPHER_SPEC_BASE=scripts/gates/cypher-size.spec.base

# split CRATE... — sets `spec` and `product` to the crates' pathspecs.
split() {
    local c s
    spec=() product=()
    for c in "$@"; do
        product+=(":(glob)crates/$c/src/**/*.rs" ":(glob)crates/$c/tests/**/*.rs")
        spec+=(":(glob)crates/$c/src/conformance/**/*.rs"
            ":(glob)crates/$c/src/**/tests.rs"
            ":(glob)crates/$c/src/**/tests/**/*.rs"
            ":(glob)crates/$c/tests/**/*.rs")
    done
    for s in "${spec[@]}"; do
        product+=(":(exclude,glob)${s#:(glob)}")
    done
}

lines() { git ls-files -z -- "$@" | xargs -0 -r cat | wc -l | tr -d ' '; }

summary=()
# budget NAME COUNT BASE_FILE CEILING CEILING_VARIABLE
budget() {
    local name="$1" count="$2" file="$3" ceiling="$4" variable="$5" base
    [ -f "$file" ] || die "$file is missing"
    base="$(tr -d '[:space:]' < "$file")"
    [[ "$base" =~ ^[0-9]+$ ]] || die "$file does not hold a number"
    [ "$count" -le "$base" ] || fail "$name is $count lines, its ratchet base is $base"
    [ "$base" -le "$ceiling" ] || fail "$name ratchet base $base is above its ceiling $ceiling"
    raised "$name" "the $name ratchet base" "$(base_number_or_zero "$file")" "$base"
    raised "$name" "the $name ceiling" "$(base_number_or_zero "scripts/gates/core-size.sh" "$variable")" "$ceiling"
    summary+=("$name $count of base $base, ceiling $ceiling")
}

# Test code in a product file would leave the specification's count.
# tests_in_product PATHSPEC...
tests_in_product() {
    git ls-files -z -- "$@" | xargs -0 -r awk '
    FNR == 1 { prev = "" }
    /^[[:space:]]*#\[(tokio::)?test/ {
        print FILENAME ":" FNR ": a test in a product file; it belongs in a tests.rs"
    }
    prev ~ /^[[:space:]]*#\[cfg\(test\)\][[:space:]]*$/ && $0 !~ /^[[:space:]]*mod tests;[[:space:]]*$/ {
        print FILENAME ":" FNR ": #[cfg(test)] stands before something other than `mod tests;`"
    }
    { prev = $0 }
'
}

split "${CRATES[@]}"
while IFS= read -r line; do fail "$line"; done < <(tests_in_product "${product[@]}")
budget product "$(lines "${product[@]}")" "$PRODUCT_BASE" "$PRODUCT_CEILING" PRODUCT_CEILING
budget specification "$(lines "${spec[@]}")" "$SPEC_BASE" "$SPEC_CEILING" SPEC_CEILING

split "${CYPHER_CRATES[@]}"
while IFS= read -r line; do fail "$line"; done < <(tests_in_product "${product[@]}")
budget "cypher product" "$(lines "${product[@]}")" "$CYPHER_PRODUCT_BASE" "$CYPHER_PRODUCT_CEILING" CYPHER_PRODUCT_CEILING
budget "cypher specification" "$(lines "${spec[@]}")" "$CYPHER_SPEC_BASE" "$CYPHER_SPEC_CEILING" CYPHER_SPEC_CEILING

verdict "$(printf '%s; ' "${summary[@]}" | sed 's/; $//')"
