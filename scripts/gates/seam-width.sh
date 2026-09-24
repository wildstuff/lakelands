#!/usr/bin/env bash
# Gate 5 — seam-width.
#
# The engine contract stays narrow, so a new engine is a port and not a
# re-implementation. Red when the methods of the traits in TRAITS, summed,
# exceed BUDGET, or when one of them is defined more than once. A method
# is an `fn` item directly in the trait body; default bodies count once.
# A trait not yet defined counts zero.
#
# The store facade trait joins TRAITS in the step that names it
# (lakelands#2).
. "$(dirname "$0")/lib.sh"

BUDGET=110
TRAITS=(LakeEngine ProjectionStore ProjectionView)

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
    [ "$defs" -le 1 ] || fail "$trait is defined $defs times; the gate cannot tell which one is the seam"
    total=$((total + count))
done
printf '%s' "$report"
[ "$total" -le "$BUDGET" ] || fail "$total methods across ${TRAITS[*]}, budget is $BUDGET"

verdict "$total of $BUDGET methods"
