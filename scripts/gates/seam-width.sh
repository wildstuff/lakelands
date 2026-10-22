#!/usr/bin/env bash
# Gate 5 — seam-width.
#
# The contract stays narrow, so a new engine is a port and not a
# re-implementation, and a caller has few doors to learn. Red when the
# methods of the traits in TRAITS, summed, exceed BUDGET, when one of
# them is defined more than once, or when one of them is not defined at
# all; and when BUDGET is higher than on the pull request's base commit
# without the row in docs/budgets.md that raises it (lib.sh, `raised`).
# A method is an `fn` item directly in the trait body; default bodies
# count once.
#
# TRAITS: the store facade (`LakeStore`, in `catalog.rs`) and every
# `pub trait` in the contract crate's `engine.rs`, read from the file, so
# a new engine trait is counted the day it lands. `Catalog` is not
# counted: it is the caller's surface, not a seam an engine ports, and
# its width is governed by the four-doors rule
# (docs/decisions/014-four-doors-per-type-one-name-generator.md). The
# seams of `seam.rs` are not counted either: each reaches outside the
# deterministic core and has a default, so a new engine ports none of
# them.
. "$(dirname "$0")/lib.sh"

BUDGET=110
ENGINE=crates/lakelands-api/src/engine.rs
[ -f "$ENGINE" ] || die "$ENGINE is missing"
mapfile -t engine_traits < <(sed -n 's/^[[:space:]]*pub trait \([A-Za-z_][A-Za-z0-9_]*\).*/\1/p' "$ENGINE")
[ "${#engine_traits[@]}" -ge 1 ] || die "$ENGINE defines no pub trait"
TRAITS=(LakeStore "${engine_traits[@]}")

total=0
report=""
for trait in "${TRAITS[@]}"; do
    defs=0
    count=0
    while IFS=' ' read -r where n; do
        defs=$((defs + 1))
        count=$((count + n))
        report+="  $trait at $where: $n"$'\n'
    done < <(git ls-files -z -- ':(glob)**/src/**/*.rs' | xargs -0 -r perl -0777 -ne '
        BEGIN { $trait = shift }
        my $src = $_;
        # One left-to-right pass, so a `//` inside a string stays a string:
        # comments vanish, string and char literals become empty.
        $src =~ s{//[^\n]*|/\*.*?\*/|("(?:\\.|[^"\\])*"|\x27(?:\\.|[^\x27\\])\x27)}
                 {defined $1 ? "\"\"" : ""}gse;
        while ($src =~ /\btrait\s+\Q$trait\E\b[^{;]*\{/g) {
            my ($depth, $n, $pos) = (1, 0, pos($src));
            while ($depth > 0 && $pos < length $src) {
                my $c = substr($src, $pos, 1);
                if    ($c eq "{") { $depth++ }
                elsif ($c eq "}") { $depth-- }
                elsif ($depth == 1 && substr($src, $pos, 2) eq "fn"
                       && substr($src, $pos + 2, 1) =~ /\s/
                       && substr($src, $pos - 1, 1) !~ /\w/) { $n++ }
                $pos++;
            }
            print "$ARGV $n\n";
        }
    ' "$trait")
    [ "$defs" -ge 1 ] || fail "$trait is not defined; the gate counts nothing for it"
    [ "$defs" -le 1 ] || fail "$trait is defined $defs times; the gate cannot tell which one is the seam"
    total=$((total + count))
done
printf '%s' "$report"
[ "$total" -le "$BUDGET" ] || fail "$total methods across ${TRAITS[*]}, budget is $BUDGET"
raised "seam width" "the seam-width budget" "$(base_number "$GATE_SELF" BUDGET)" "$BUDGET"

verdict "$total of $BUDGET methods"
