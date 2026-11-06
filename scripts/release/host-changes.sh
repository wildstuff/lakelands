#!/usr/bin/env bash
# Lists the changes a host must make to its own code when it moves from
# a baseline tag to this tree: one plain line each, for the crates a host
# builds against. Under 0.x a release may break a host; it must say how.
# Usage: scripts/release/host-changes.sh [<baseline-tag>]
#   (default: the nearest `v*` tag that is not a pre-release)
#        scripts/release/host-changes.sh --reach <file> <item>
#   (the module path a host names the item of the file by, or that none
#   reaches it: what host-changes-test.sh holds `reach` by)
#        scripts/release/host-changes.sh --own <baseline-tag>
#   (the script's own checks alone, without the tool: one line a change,
#   the crate, the check and the item apart by tabs; what
#   scripts/release/surface-held.sh judges beside the tool's findings)
#        scripts/release/host-changes.sh --compare <was.rs> <now.rs>
#   (the script's own three comparisons over two files and nothing else:
#   no baseline and no tool; what `host-changes-test.sh` holds it by)
#
# The findings are cargo-semver-checks' (checked against the rules of a
# minor release: under 0.x the tool reads 0.5 -> 0.6 as a major step and
# would report nothing), plus checks of its own that the tool does not
# make, over the same crates: over their public traits, a method without
# a default that was added, which every implementer must add, and a
# method whose signature changed while its parameter count did not,
# which every implementer must follow; a public function or inherent
# method whose signature changed while its parameter count did not, which
# every call must follow; and a field of a public struct or of a public
# enum's variant whose type changed, named (`Type.field`) or positional
# (`Type.0`), the struct or variant written over several lines or on one,
# which a literal, a pattern or a read of it must follow. An
# item counts as public when every module on its way from the crate root
# is `pub mod`, or a private one re-exports it with `pub use`, and neither
# the item nor a module or re-export on its way is `#[doc(hidden)]`: a
# hidden item is the workspace's, no host's, as `cargo semver-checks`
# reads it. Only
# changes count: an addition asks nothing of a host, so it is not listed.
# An item a line already names is not reported twice. With nothing found
# it says so. It exits 0 whatever it finds, and 2 when it cannot run.
#
# In all three of its own comparisons (a trait method's signature, a
# function's or method's, a field's type) a type written by another path
# is the same type: `lakelands_api::ontology::FieldDecl` and `FieldDecl`
# agree, as two paths do whenever one is the other's ending at a `::`.
# `a::Decl` and `b::Decl` do not, and are reported.
#
# Not compared, so a release reviewer reads them by hand: the items of a
# public struct, enum, trait or impl block declared anywhere but at
# column 0; the fields of a struct whose `{` stands on a line after its
# name's (under a `where` clause of its own lines), and of an enum
# written whole on one line, which `rustfmt` never writes; a method whose impl block lies in a private module that does
# not re-export its type; `pub(crate)` items, which reach no host; and a
# path written shorter whose shorter form an import now binds to another
# type of that name (`std::result::Result<T>` and a crate's own
# `Result<T>` read as one), since the imports are not resolved; by the
# same rule `Self::Out` and a type named `Out` read as one.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

CRATES=(lakelands-api lakelands-client lakelands-core lakelands-store
    lakelands-engine-mem lakelands-engine-sqlite lakelands-engine-pg)

# `--compare` asks the tool nothing and reads no crate.
compare=()
own=false
reaching=()
if [ "${1:-}" = --own ]; then own=true; shift; fi
if [ "${1:-}" = --reach ]; then
    [ -f "${2:-}" ] && [ -n "${3:-}" ] ||
        { echo "host-changes: cannot run: --reach takes a file and an item" >&2; exit 2; }
    reaching=("$2" "$3"); set -- --compare "$2" "$2"
fi
if [ "${1:-}" = --compare ]; then
    [ -f "${2:-}" ] && [ -f "${3:-}" ] ||
        { echo "host-changes: cannot run: --compare takes two files" >&2; exit 2; }
    compare=("$2" "$3") CRATES=()
else
    baseline="${1:-$(git describe --tags --abbrev=0 --match 'v*' --exclude 'v*-*' HEAD 2>/dev/null || true)}"
    [ -n "$baseline" ] || { echo "host-changes: cannot run: no baseline tag" >&2; exit 2; }
    git rev-parse -q --verify "$baseline^{commit}" >/dev/null ||
        { echo "host-changes: cannot run: $baseline is not a revision" >&2; exit 2; }
    $own || command -v cargo-semver-checks >/dev/null ||
        { echo "host-changes: cannot run: cargo-semver-checks is not installed" >&2; exit 2; }
fi

# One sentence per lint: what a host has to do. A lint not named here
# keeps the tool's own summary, so nothing found is ever dropped.
advice() {
    case "$1" in
    enum_variant_added) echo "an exhaustive \`match\` on it must handle the new variant" ;;
    enum_struct_variant_field_added) echo "a literal or pattern of the variant without \`..\` must name the new field" ;;
    constructible_struct_adds_field) echo "a struct literal without \`..Default::default()\` must set the new field" ;;
    struct_pub_field_missing | struct_missing | enum_missing | function_missing | trait_missing | enum_variant_missing | \
        inherent_method_missing)
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
rows=()
$own && tool=() || tool=("${CRATES[@]}")
for crate in "${tool[@]}"; do
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
            # A path with `::` throughout: the tool writes an added
            # variant with one colon (`Func:Retired`).
            while (match(item, /[A-Za-z0-9_]:[A-Za-z_]/))
                item = substr(item, 1, RSTART) ":" substr(item, RSTART + 1)
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

# Shared by the readers below. `norm` collapses the whitespace of a
# signature or a type and drops rustfmt's trailing comma of the vertical
# form, so the one-line and the vertical form read the same. `unnamed`
# leaves out the parameter names of a signature (a `name: ` or
# `mut name: ` at the start of a parameter of the outer list), which is
# what is compared: a renamed parameter asks nothing of a caller or an
# implementation. `depth` counts the brackets a text leaves open. `same`
# says whether two signatures or types agree once a path may be written
# shorter: token by token, where a token is a path or one other character
# and two paths agree when one is the other or its ending at a `::`.
LIB='
function norm(s) {
    gsub(/[ \t]+/, " ", s)
    sub(/^ /, "", s); sub(/ $/, "", s)
    gsub(/\( /, "(", s); gsub(/ \)/, ")", s); gsub(/< /, "<", s); gsub(/ >/, ">", s)
    gsub(/\[ /, "[", s); gsub(/ \]/, "]", s)
    gsub(/,\)/, ")", s); gsub(/,>/, ">", s)
    return s
}
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
function tokens(s, out,    n, m) {
    n = 0
    while (length(s) > 0) {
        m = match(s, /^[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*/) ? RLENGTH : 1
        out[++n] = substr(s, 1, m); s = substr(s, m + 1)
    }
    return n
}
function ending(long, short) {
    return long == short || (length(long) > length(short) + 2 &&
        substr(long, length(long) - length(short) - 1) == "::" short)
}
function same(a, b,    x, y, n, i) {
    if (a == b) return 1
    n = tokens(a, x)
    if (n != tokens(b, y)) return 0
    for (i = 1; i <= n; i++) if (!ending(x[i], y[i]) && !ending(y[i], x[i])) return 0
    return 1
}
function depth(s,    i, n, c, d) {
    d = 0; n = length(s)
    for (i = 1; i <= n; i++) {
        c = substr(s, i, 1)
        if (c == "-" && substr(s, i + 1, 1) == ">") { i++; continue }
        if (c == "(" || c == "[" || c == "<") d++
        else if (c == ")" || c == "]" || c == ">") d--
    }
    return d
}
'

# The lines whose tab-separated fields `$1` and `$2` (by number) do not
# hold the same signature or type: what the three comparisons below keep.
changed() {
    awk -F '\t' -v a="$1" -v b="$2" "$LIB"'!same($a, $b)'
}

# Every public trait (`Trait`, `trait`) and every method of one, one line
# each, fields apart by a tab: `Trait::method`, `required` or `provided`
# (a default body), its signature as written, and the same without
# parameter names. A signature is read from its `fn` (or `async fn`) at
# the trait's indent to its `;` or the `{` of its body, without comments
# or attributes.
methods() {
    awk "$LIB"'
        function done(kind) {
            sig = norm(sig)
            print t "::" name "\t" kind "\t" sig "\t" unnamed(sig)
            sig = ""; reading = 0
        }
        /^[ \t]*#\[doc\(hidden\)\][ \t]*$/ { hid = 1; next }
        /^[ \t]*(#\[|\/\/)/ && !reading { next }
        { h = hid; if (!reading) hid = 0 }
        /^pub trait [A-Za-z]/ && h { intrait = 0; next }
        /^pub trait [A-Za-z]/ {
            t = $3; sub(/[:<{].*/, "", t); intrait = 1
            print t "\ttrait\t\t"
            next
        }
        !intrait { next }
        /^\}/ { intrait = 0; reading = 0; next }
        !reading && h && /^    (async )?fn [A-Za-z_]/ { next }
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

# Every public function at column 0 (`name`) and every public method of
# an inherent impl block at column 0 (`Type::name`), one line each,
# fields apart by a tab: the name, its signature as written and the same
# without parameter names, read as for a trait method.
functions() {
    awk "$LIB"'
        function done() {
            sig = norm(sig)
            print name "\t" sig "\t" unnamed(sig)
            sig = ""; reading = 0
        }
        /^[ \t]*#\[doc\(hidden\)\][ \t]*$/ { hid = 1; next }
        /^[ \t]*(#\[|\/\/)/ && !reading { next }
        { h = hid; if (!reading) hid = 0 }
        /^impl[ <]/ && !/ for / {
            ty = $0; sub(/^impl/, "", ty)
            # Past the impl block`s own generics, which may nest.
            if (ty ~ /^</) {
                n = length(ty); d = 0
                for (i = 1; i <= n; i++) {
                    c = substr(ty, i, 1)
                    if (c == "<") d++
                    else if (c == ">" && substr(ty, i - 1, 1) != "-" && --d == 0) break
                }
                ty = substr(ty, i + 1)
            }
            sub(/^ +/, "", ty); sub(/[^A-Za-z0-9_].*/, "", ty)
            inimpl = 1; next
        }
        /^\}/ { inimpl = 0; reading = 0; next }
        !reading && h && /^(    )?pub ((const|async|unsafe) )*fn [A-Za-z_]/ { next }
        !reading && !inimpl && /^pub ((const|async|unsafe) )*fn [A-Za-z_]/ {
            name = $0; sub(/^pub ((const|async|unsafe) )*fn /, "", name); sub(/[^A-Za-z0-9_].*/, "", name)
            reading = 1; sig = ""
        }
        !reading && inimpl && /^    pub ((const|async|unsafe) )*fn [A-Za-z_]/ {
            name = $0; sub(/^    pub ((const|async|unsafe) )*fn /, "", name); sub(/[^A-Za-z0-9_].*/, "", name)
            name = ty "::" name
            reading = 1; sig = ""
        }
        reading {
            line = $0
            if (line ~ /^[ \t]*(#\[|\/\/)/) next
            sub(/[ \t]*\/\/.*$/, "", line)
            sig = sig " " line
            if (sig ~ /\{/) { sub(/[ \t]*\{.*$/, "", sig); done() }
            else if (sig ~ /;[ \t]*$/) { sub(/;[ \t]*$/, "", sig); done() }
        }
    '
}

# Every field of a public struct (its `pub` fields) and of a public
# enum's variants, one line each, fields apart by a tab: `Type.field` or
# `Enum::Variant.field`, positional ones by their place (`Type.0`,
# `Enum::Variant.1`), then its type. A type runs to the comma that ends
# it outside every bracket, over as many lines as it takes; comments and
# attribute lines are left out, and the whitespace is collapsed as for a
# signature.
fields() {
    awk "$LIB"'
        function done() {
            ty = typ; sub(/,[ \t]*$/, "", ty)
            print field "\t" norm(ty)
            reading = 0; typ = ""
        }
        # The text between the first `open` of s, `(` or `{`, and the
        # bracket that closes it.
        function inner(s, open,    i, n, c, d, from) {
            n = length(s); d = 0; from = index(s, open)
            for (i = from; i <= n; i++) {
                c = substr(s, i, 1)
                if (c == "-" && substr(s, i + 1, 1) == ">") { i++; continue }
                if (c == "(" || c == "[" || c == "<" || c == "{") d++
                else if (c == ")" || c == "]" || c == ">" || c == "}") d--
                if (d == 0) break
            }
            return substr(s, from + 1, i - from - 1)
        }
        # One line per place of a tuple, or with `braced` per field of a
        # struct or variant written on one line; for a struct, only its
        # `pub` places or fields.
        function places(prefix, s, pubonly, braced,    i, n, c, d, part, pos) {
            s = inner(s, braced ? "{" : "("); n = length(s); d = 0; part = ""; pos = 0
            for (i = 1; i <= n; i++) {
                c = substr(s, i, 1)
                if (c == "-" && substr(s, i + 1, 1) == ">") { part = part "->"; i++; continue }
                if (c == "(" || c == "[" || c == "<") d++
                else if (c == ")" || c == "]" || c == ">") d--
                if (c == "," && d == 0) { place(prefix, part, pos++, pubonly, braced); part = ""; continue }
                part = part c
            }
            if (part !~ /^[ \t]*$/) place(prefix, part, pos, pubonly, braced)
            tuple = 0; tbuf = ""
        }
        function place(prefix, part, pos, pubonly, braced,    name) {
            part = norm(part)
            if (pubonly) { if (part !~ /^pub /) return; sub(/^pub /, "", part) }
            if (braced) { name = part; sub(/:.*/, "", name); sub(/^[^:]*: */, "", part); pos = name }
            print prefix "." pos "\t" part
        }
        /^[ \t]*#\[doc\(hidden\)\][ \t]*$/ { hid = 1; next }
        {
            line = $0
            if (line ~ /^[ \t]*(#\[|\/\/)/) next
            sub(/[ \t]*\/\/.*$/, "", line)
            h = hid; if (!reading && !tuple) hid = 0
        }
        tuple {
            tbuf = tbuf " " line
            if (depth(tbuf) <= 0) places(tprefix, tbuf, tpub, 0)
            next
        }
        !inside && h && /^pub (struct|enum) [A-Za-z_]/ { if ($0 ~ /\{[ \t]*$/) skip = 1; next }
        skip { if ($0 ~ /^\}/) skip = 0; next }
        line ~ /^pub struct [A-Za-z_][A-Za-z0-9_]*(<.*>)? *\{.*\}[ \t]*$/ {
            item = $3; sub(/[^A-Za-z0-9_].*/, "", item); places(item, line, 1, 1); next
        }
        /^pub struct [A-Za-z_]/ && /\{[ \t]*$/ {
            item = $3; sub(/[^A-Za-z0-9_].*/, "", item); kind = "struct"; inside = 1; next
        }
        /^pub struct [A-Za-z_][A-Za-z0-9_]*(<.*>)?\(/ {
            tprefix = $3; sub(/[^A-Za-z0-9_].*/, "", tprefix); tpub = 1
            tbuf = substr(line, index(line, "(")); tuple = 1
            if (depth(tbuf) <= 0) places(tprefix, tbuf, tpub, 0)
            next
        }
        /^pub enum [A-Za-z_]/ && /\{[ \t]*$/ {
            item = $3; sub(/[^A-Za-z0-9_].*/, "", item); kind = "enum"; inside = 1; variant = ""; next
        }
        !inside { next }
        /^\}/ { inside = 0; variant = ""; reading = 0; next }
        reading {
            typ = typ " " line
            if (depth(typ) == 0 && typ ~ /,[ \t]*$/) done()
            next
        }
        kind == "struct" && line ~ /^    pub [a-z_][a-z0-9_]*: / {
            field = line; sub(/^    pub /, "", field); sub(/:.*/, "", field); field = item "." field
            typ = line; sub(/^    pub [a-z_][a-z0-9_]*: /, "", typ); reading = 1
            if (depth(typ) == 0 && typ ~ /,[ \t]*$/) done()
            next
        }
        kind == "enum" && line ~ /^    [A-Z][A-Za-z0-9_]* \{.*\},?[ \t]*$/ {
            tprefix = line; sub(/^    /, "", tprefix); sub(/ .*/, "", tprefix)
            places(item "::" tprefix, line, 0, 1); next
        }
        kind == "enum" && line ~ /^    [A-Z][A-Za-z0-9_]* \{[ \t]*$/ {
            variant = line; sub(/^    /, "", variant); sub(/ .*/, "", variant); next
        }
        kind == "enum" && line ~ /^    [A-Z][A-Za-z0-9_]*\(/ {
            tprefix = line; sub(/^    /, "", tprefix); sub(/\(.*/, "", tprefix)
            tprefix = item "::" tprefix; tpub = 0
            tbuf = substr(line, index(line, "(")); tuple = 1
            if (depth(tbuf) <= 0) places(tprefix, tbuf, tpub, 0)
            next
        }
        kind == "enum" && line ~ /^    \}/ { variant = ""; next }
        kind == "enum" && variant != "" && line ~ /^        [a-z_][a-z0-9_]*: / {
            field = line; sub(/^        /, "", field); sub(/:.*/, "", field)
            field = item "::" variant "." field
            typ = line; sub(/^        [a-z_][a-z0-9_]*: /, "", typ); reading = 1
            if (depth(typ) == 0 && typ ~ /,[ \t]*$/) done()
            next
        }
    '
}

# The module path a host names an item of a source file by, from the
# crate root (empty for the root itself): every module on the way is
# `pub mod`, or a private `mod` whose parent re-exports the item with a
# `pub use` of it (or of a glob), which takes that module out of the path.
# A `pub mod` under `#[doc(hidden)]` is passed as a private one is: only a
# re-export not itself hidden reaches a host through it. Fails when the
# item is not reachable. Read from this tree.
reach() {
    local file="$1" item="$2" src rel dir="" path="" decl seg
    src="${file%%/src/*}/src"
    rel="${file#"$src"/}"; rel="${rel%.rs}"; rel="${rel%/mod}"
    [ "$rel" = lib ] && rel=""
    local IFS=/
    for seg in $rel; do
        if [ -z "$dir" ]; then decl="$src/lib.rs"
        elif [ -f "$src/$dir.rs" ]; then decl="$src/$dir.rs"
        else decl="$src/$dir/mod.rs"; fi
        if grep -Eq "^[[:space:]]*pub mod $seg;" "$decl" && ! awk -v m="$seg" '
                /^[ \t]*#\[doc\(hidden\)\][ \t]*$/ { hid = 1; next }
                /^[ \t]*(#\[|\/\/)/ { next }
                $0 ~ "^[ \t]*pub mod " m ";" { found = hid; exit }
                { hid = 0 }
                END { exit !found }' "$decl"; then
            path="${path:+$path::}$seg"
        elif ! awk '/^[ \t]*#\[doc\(hidden\)\][ \t]*$/ { hid = 1; next }
                /^[ \t]*(#\[|\/\/)/ && !on { next }
                /^[ \t]*pub use / && !hid { s = ""; on = 1 } !on { hid = 0 }
                on { s = s " " $0 } on && /;/ { print s; on = 0; hid = 0 }' "$decl" |
            grep -E "use $seg::" | sed -E "s/.*use $seg:://" |
            grep -Eq "(^|[^A-Za-z0-9_])$item([^A-Za-z0-9_]|$)|^\*"; then
            return 1
        fi
        dir="${dir:+$dir/}$seg"
    done
    printf '%s\n' "$path"
}

if [ "${#reaching[@]}" -gt 0 ]; then
    reach "${reaching[0]}" "${reaching[1]}" || echo "(no host reaches it)"
    exit 0
fi

# Whether a line of the report already names `Trait::method`, `Type.field`
# or a function's path, and not only a longer name it begins (`get` in
# `get_many`). With `qualified`, a name with a module path before it
# counts too (`lake::Staged::get` for `Staged::get`).
named() {
    local before='(^|[^A-Za-z0-9_:.])'
    [ "${2:-}" = qualified ] && before='(^|[^A-Za-z0-9_:.]|::)'
    [ "${#lines[@]}" -gt 0 ] &&
        grep -Eq -- "$before${1//./\\.}([^A-Za-z0-9_]|$)" <<<"$(printf '%s\n' "${lines[@]}")"
}

# The three comparisons, each over what one file held and what it holds:
# the items in both whose signature or type is not the same, one line
# each, fields apart by a tab. `method_rows` takes the two outputs of
# `methods`; the other two the baseline's text and the file.
method_rows() {
    LC_ALL=C join -t $'\t' -o 1.1,1.3,1.4,2.3,2.4 \
        <(printf '%s\n' "$1" | sed '/^$/d' | LC_ALL=C sort -t $'\t' -k1,1) \
        <(printf '%s\n' "$2" | sed '/^$/d' | LC_ALL=C sort -t $'\t' -k1,1) | changed 3 5
}
function_rows() {
    LC_ALL=C join -t $'\t' -o 1.1,1.2,1.3,2.2,2.3 \
        <(printf '%s\n' "$1" | functions | LC_ALL=C sort -t $'\t' -k1,1) \
        <(functions < "$2" | LC_ALL=C sort -t $'\t' -k1,1) | changed 3 5
}
field_rows() {
    LC_ALL=C join -t $'\t' -o 1.1,1.2,2.2 \
        <(printf '%s\n' "$1" | fields | LC_ALL=C sort -t $'\t' -k1,1) \
        <(fields < "$2" | LC_ALL=C sort -t $'\t' -k1,1) | changed 2 3
}

if [ "${#compare[@]}" -gt 0 ]; then
    was_src="$(cat "${compare[0]}")"
    method_rows "$(printf '%s\n' "$was_src" | methods)" "$(methods < "${compare[1]}")" |
        awk -F '\t' '{ print "trait method " $1 ": was `" $2 "`, now `" $4 "`" }'
    function_rows "$was_src" "${compare[1]}" |
        awk -F '\t' '{ print "function or method " $1 ": was `" $2 "`, now `" $4 "`" }'
    field_rows "$was_src" "${compare[1]}" |
        awk -F '\t' '{ print "field " $1 ": was `" $2 "`, now `" $3 "`" }'
    exit 0
fi

# The items in both trees, compared file by file. Test files hold no
# public items.
while read -r f; do
    [ -f "$f" ] || continue
    case "$f" in */tests.rs | */tests/*) continue ;; esac
    crate="${f#crates/}"; crate="${crate%%/*}"
    was_src="$(git show "$baseline:$f" 2>/dev/null)" || was_src=""

    base="$(printf '%s\n' "$was_src" | methods)"
    head="$(methods < "$f")"
    # Required now and not in the baseline, in a trait the baseline has:
    # added, or its default dropped. A new trait asks nothing of anyone.
    while read -r m; do
        [ -n "$m" ] || continue
        printf '%s\n' "$base" | awk -F '\t' -v t="${m%%::*}" '$1 == t && $2 == "trait" { f = 1 } END { exit !f }' ||
            continue
        reach "$f" "${m%%::*}" >/dev/null || continue
        named "$m" qualified && continue
        lines+=("\`$crate\`: trait method $m: an implementation of the trait must add it (trait_method_added)")
        rows+=("$crate"$'\t'trait_method_added$'\t'"trait method $m")
    done < <(LC_ALL=C comm -13 \
        <(printf '%s\n' "$base" | awk -F '\t' '$2 == "required" { print $1 }' | LC_ALL=C sort -u) \
        <(printf '%s\n' "$head" | awk -F '\t' '$2 == "required" { print $1 }' | LC_ALL=C sort -u))
    # In both, with another signature once the parameter names are left
    # out. Under `async_trait` an `async` that comes or goes is a change.
    while IFS=$'\t' read -r m was was_unnamed now now_unnamed; do
        reach "$f" "${m%%::*}" >/dev/null || continue
        named "$m" qualified && continue
        lines+=("\`$crate\`: trait method $m: an implementation of the trait must follow: was \`$was\`, now \`$now\` (trait_method_signature_changed)")
        rows+=("$crate"$'\t'trait_method_signature_changed$'\t'"trait method $m")
    done < <(method_rows "$base" "$head")

    # A public function or inherent method in both, with another
    # signature once the parameter names are left out: a function is
    # named by its path, as the tool names it, a method by its type.
    while IFS=$'\t' read -r m was was_unnamed now now_unnamed; do
        path="$(reach "$f" "${m%%::*}")" || continue
        case "$m" in
        *::*) k="$m" what=method lint=method_signature_changed ;;
        *) k="${crate//-/_}::${path:+$path::}$m" what=function lint=function_signature_changed ;;
        esac
        named "$k" qualified && continue
        lines+=("\`$crate\`: $what $k: every call must follow: was \`$was\`, now \`$now\` ($lint)")
        rows+=("$crate"$'\t'"$lint"$'\t'"$what $k")
    done < <(function_rows "$was_src" "$f")

    # A field in both, with another type: a host that builds, matches or
    # reads it must follow.
    while IFS=$'\t' read -r k was now; do
        t="${k%%.*}"
        reach "$f" "${t%%::*}" >/dev/null || continue
        named "$k" && continue
        # The tool names a variant's field `field <f> of variant <E>::<V>`.
        case "$k" in
        *::*) named "${k##*.} of variant ${k%.*}" && continue ;;
        esac
        case "$k" in
        *::*) lint=enum_variant_field_type_changed ;;
        *) lint=struct_field_type_changed ;;
        esac
        lines+=("\`$crate\`: field $k: a literal, a pattern or a read of the field must follow: was \`$was\`, now \`$now\` ($lint)")
        rows+=("$crate"$'\t'"$lint"$'\t'"field $k")
    done < <(field_rows "$was_src" "$f")
done < <(git ls-tree -r --name-only "$baseline" -- "${CRATES[@]/#/crates/}" | grep '/src/.*\.rs$')

if $own; then
    [ "${#rows[@]}" -eq 0 ] || printf '%s\n' "${rows[@]}" | LC_ALL=C sort -u
    exit 0
fi

echo "Changes a host must make, since $baseline:"
echo
if [ "${#lines[@]}" -eq 0 ]; then
    echo "- No change a host must make."
else
    printf '%s\n' "${lines[@]}" | sort -u | sed 's/^/- /'
fi
