#!/usr/bin/env bash
# Gate 18 — rights-private.
#
# The rules that judge a principal are reached through its `Sight`, so a
# door cannot ask a rule with a principal of its own making or skip the
# one it should have asked. `lakelands_core::rights` is crate-private and
# stays so. Red when:
#   - `crates/lakelands-core/src/lib.rs` declares `rights` other than as
#     `pub(crate) mod rights;` or `mod rights;`;
#   - a file of the core crate re-exports the module itself, whole
#     (`pub use …rights;`, `pub use …rights as …`) or by a glob
#     (`pub use …rights::*`); naming the items it re-exports is fine;
#   - any `.rs` outside `crates/lakelands-core/src/` names a `rights::`
#     path, or imports `rights` from `lakelands_core` in a braced list.
# Untracked files are checked too, so a local run sees new work.
. "$(dirname "$0")/lib.sh"

CORE='crates/lakelands-core/src'
LIB="$CORE/lib.rs"

[ -f "$LIB" ] || die "$LIB is missing"
decl="$(grep -cE '^\s*mod rights;$|^\s*pub\(crate\) mod rights;$' "$LIB" || true)"
[ "$decl" = 1 ] || fail "$LIB: \`rights\` is not declared once as \`pub(crate) mod rights;\`"

files=0
while IFS= read -r -d '' file; do
    files=$((files + 1))
    case "$file" in
        "$CORE"/*) pattern='^\s*pub(?:\([^)]*\))?\s+use\s+[\w:]*\brights\s*(?:;|\s+as\b|::\s*\*)' ;;
        *) pattern='\brights::|lakelands_core::\{[^}]*\brights\b' ;;
    esac
    while IFS= read -r hit; do fail "$hit"; done < <(perl -ne '
        BEGIN { ($pattern, $file) = splice @ARGV, 0, 2 }
        print "$file:$.: names the crate-private `rights` module\n" if /$pattern/;
    ' "$pattern" "$file" "$file")
done < <(git ls-files -z --cached --others --exclude-standard -- ':(glob)**/*.rs')

verdict "$files Rust file(s), \`rights\` private to $CORE"
