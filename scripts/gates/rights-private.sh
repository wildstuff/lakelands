#!/usr/bin/env bash
# Gate 18 — rights-private.
#
# The rules that judge a principal are reached through its `Sight`, so a
# door cannot ask a rule with a principal of its own making or skip the
# one it should have asked. `lakelands_core::rights` is crate-private and
# stays so, and every rule of it that judges a principal is `pub(crate)`:
# the compiler refuses a re-export of one. Red when:
#   - `crates/lakelands-core/src/lib.rs` declares `rights` other than as
#     `pub(crate) mod rights;` or `mod rights;`;
#   - a file of the core crate re-exports out of `rights` with `pub use`
#     the module itself (whole or by `as`), a glob of it, or an item
#     ALLOWED does not name, a braced list over several lines included;
#   - any `.rs` outside `crates/lakelands-core/src/` names a `rights::`
#     path, or imports `rights` from `lakelands_core` in a braced list on
#     one line (the private module refuses the other forms at compile
#     time);
#   - any `.rs` but `sight.rs` and `rights.rs` reads `grants.restricted`:
#     whether a principal is shown held records is asked of its sight
#     (`Sight::sees_restricted`). Setting the grant, `….grants.restricted
#     = …` or as the last place of a tuple assigned, is no read. The rule
#     reads text: a destructured `Grants { restricted, .. }` or a read
#     through another name for the grants passes it, and is review's.
#     `MIRROR` (below) is exempt: it compares the grants two connections
#     are configured with before any door is asked, and holds no sight.
# Untracked files are checked too, so a local run sees new work.
. "$(dirname "$0")/lib.sh"

CORE='crates/lakelands-core/src'
LIB="$CORE/lib.rs"
MIRROR='crates/lakelands-cli/src/mirror.rs'
# What a door may name out of `rights`: the rules that judge no principal
# and the types it names beside its sight.
ALLOWED='check_admitted is_system not_a_view Admit Door Find'

[ -f "$LIB" ] || die "$LIB is missing"
decl="$(grep -cE '^\s*mod rights;$|^\s*pub\(crate\) mod rights;$' "$LIB" || true)"
[ "$decl" = 1 ] || fail "$LIB: \`rights\` is not declared once as \`pub(crate) mod rights;\`"

files=0
while IFS= read -r -d '' file; do
    files=$((files + 1))
    case "$file" in
        "$CORE"/*) while IFS= read -r hit; do fail "$hit"; done < <(perl -0777 -ne '
            BEGIN { ($allowed, $file) = splice @ARGV, 0, 2; %ok = map { $_ => 1 } split " ", $allowed }
            while (/^[ \t]*pub[ \t]+use[ \t]+((?:crate|self|super)::)*rights\b([^;]*);/mg) {
                my ($line, $rest) = (1 + (substr($_, 0, $-[0]) =~ tr/\n//), $2);
                my @items = $rest =~ /^\s*::\s*\{(.*)\}\s*$/s ? split /\s*,\s*/, $1
                    : $rest =~ /^\s*::\s*(\w+(?:\s+as\s+\w+)?)\s*$/ ? ($1) : ();
                print "$file:$line: re-exports the `rights` module, whole or by a glob\n" unless @items;
                for (@items) {
                    s/^\s+|\s+$//g;
                    next if $_ eq "";
                    print "$file:$line: re-exports `rights::$_`, which ALLOWED does not name\n"
                        unless $ok{$_};
                }
            }
        ' "$ALLOWED" "$file" "$file") ;;
        *) while IFS= read -r hit; do fail "$hit"; done < <(perl -ne '
            BEGIN { $file = shift }
            print "$file:$.: names the crate-private `rights` module\n"
                if /\brights::|lakelands_core::\{[^}]*\brights\b/;
        ' "$file" "$file") ;;
    esac
    case "$file" in "$CORE"/sight.rs|"$CORE"/rights.rs|"$MIRROR") continue ;; esac
    while IFS= read -r hit; do fail "$hit"; done < <(perl -ne '
        BEGIN { $file = shift }
        print "$file:$.: reads `grants.restricted`; ask the sight (`Sight::sees_restricted`)\n"
            if /\bgrants\.restricted\b(?!\s*=[^=])(?!\)\s*=[^=])/;
    ' "$file" "$file")
done < <(git ls-files -z --cached --others --exclude-standard -- ':(glob)**/*.rs')

verdict "$files Rust file(s), \`rights\` private to $CORE"
