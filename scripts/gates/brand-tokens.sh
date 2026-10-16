#!/usr/bin/env bash
# Gate 17 — brand-tokens.
#
# brand/tokens.css is generated from brand/tokens.json by
# scripts/brand/tokens-css.pl. A colour changed in only one of the two
# files would drift silently: the stylesheet would say one thing and
# every figure built from the tokens another. Red when the committed
# stylesheet is not, byte for byte, what the generator makes from the
# committed tokens.
. "$(dirname "$0")/lib.sh"

[ -f brand/tokens.json ] || die "brand/tokens.json is missing"
[ -f brand/tokens.css ] || die "brand/tokens.css is missing"

generated="$(mktemp)"
trap 'rm -f "$generated"' EXIT
perl scripts/brand/tokens-css.pl brand/tokens.json > "$generated" \
    || die "scripts/brand/tokens-css.pl could not read brand/tokens.json"

if ! cmp -s "$generated" brand/tokens.css; then
    fail "brand/tokens.css is not what scripts/brand/tokens-css.pl makes from brand/tokens.json; regenerate it: perl scripts/brand/tokens-css.pl brand/tokens.json > brand/tokens.css"
fi

verdict "brand/tokens.css matches brand/tokens.json"
