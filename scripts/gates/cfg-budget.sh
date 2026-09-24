#!/usr/bin/env bash
# Gate 2 — cfg-budget.
#
# Cargo features are one per engine and nothing else: no test-support
# feature, no capability flags. Red when
#   - the workspace has more than BUDGET `cfg(feature …)` sites — every
#     `cfg(…)`, `cfg!(…)` or `cfg_attr(…)` whose condition names a
#     feature counts once;
#   - a manifest declares a feature that is not an engine name (the
#     engine crates are `lakelands-engine-<name>`), or `default`;
#   - an optional dependency is not hidden behind `dep:` (it would become
#     an implicit feature of its own).
. "$(dirname "$0")/lib.sh"

BUDGET=20

engines="$(git ls-files 'crates/lakelands-engine-*/Cargo.toml' \
    | sed -E 's#^crates/lakelands-engine-([^/]+)/Cargo.toml$#\1#' | sort -u | tr '\n' ' ')"
[ -n "$engines" ] || die "no engine crate found under crates/"

sites=0
while IFS= read -r -d '' file; do
    while IFS= read -r site; do
        sites=$((sites + 1))
        echo "  $site"
    done < <(perl -0777 -ne '
        my $file = $ARGV;
        while (/\bcfg(?:_attr)?!?\s*(\((?:[^()]++|(?1))*\))/g) {
            my ($cond, $at) = ($1, $-[0]);
            next unless $cond =~ /\bfeature\s*=/;
            my $line = 1 + (substr($_, 0, $at) =~ tr/\n//);
            print "$file:$line\n";
        }
    ' "$file")
done < <(tracked_files '*.rs')
[ "$sites" -le "$BUDGET" ] || fail "$sites cfg(feature) sites, budget is $BUDGET"

while IFS= read -r -d '' manifest; do
    while IFS= read -r msg; do fail "$manifest: $msg"; done < <(perl -ne '
        BEGIN { %engine = map { $_ => 1 } split " ", shift; }
        if (/^\s*\[(.+?)\]\s*$/) { $section = $1; next }
        if ($section eq "features" && /^\s*"?([A-Za-z0-9_.-]+)"?\s*=/) {
            print "feature `$1` is not an engine (allowed: default, ",
                join(", ", sort keys %engine), ")\n"
                unless $1 eq "default" || $engine{$1};
        }
        $optional{$1} = 1 if /^\s*([A-Za-z0-9_-]+)\s*=\s*\{.*\boptional\s*=\s*true/;
        $optional{$1} = 1 if $section =~ /dependencies\.([A-Za-z0-9_-]+)$/ && /^\s*optional\s*=\s*true/;
        $hidden{$1} = 1 while /"dep:([A-Za-z0-9_-]+)"/g;
        END {
            print "optional dependency `$_` is an implicit feature; name it `dep:$_` under an engine feature\n"
                for grep { !$hidden{$_} } sort keys %optional;
        }
    ' "$engines" "$manifest")
done < <(tracked_files 'Cargo.toml' '*/Cargo.toml')

verdict "$sites of $BUDGET cfg(feature) sites; engines: ${engines% }"
