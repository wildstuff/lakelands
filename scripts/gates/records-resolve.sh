#!/usr/bin/env bash
# Gate 13 — records resolve.
#
# The code, the pages and the records cite a decision record by its
# path, and a record that moved into another keeps its file and points
# at the rule that holds what it held (`docs/decisions/README.md`). Red
# when a `docs/decisions/NNN-<slug>.md` path anywhere in the tree names
# no file; when a link from one record to another names no file; and
# when a pointer, written `NNN § <heading>`, names a record that is not
# there or a heading that record does not have. A pointer stands in a
# code span: of two backticks where the heading holds a code span
# itself, of one otherwise.
#
# Not read, by design: a record cited by its bare number, a link written
# with `./` or from a page outside the records, and a link's anchor.
. "$(dirname "$0")/lib.sh"

DIR=docs/decisions
paths=0

# A path citation, anywhere in the tree.
while IFS=: read -r file line cited; do
    [ -n "$file" ] || continue
    paths=$((paths + 1))
    [ -f "$cited" ] || fail "$file:$line cites $cited, and there is no such record"
done < <(tracked_files | xargs -0 grep -noE "$DIR/[0-9]{3}-[a-z0-9-]+\.md" -- 2>/dev/null || true)

# A link from a record to a record beside it.
while IFS=: read -r file line target; do
    [ -n "$file" ] || continue
    target="${target#](}"
    [ -f "$DIR/${target%%#*}" ] || fail "$file:$line links to ${target%%#*}, and there is no such record"
done < <(git grep -noE '\]\([0-9]{3}-[a-z0-9-]+\.md(#[^)]*)?' -- "$DIR" || true)

# A pointer: `NNN § <heading>` names a heading of record NNN.
pointers=0
while IFS=: read -r file line pointer; do
    [ -n "$file" ] || continue
    pointers=$((pointers + 1))
    # The span's backticks, and the space that parts a closing pair from
    # a code span the heading ends in.
    case "$pointer" in
        '``'*) pointer="${pointer#\`\`}"; pointer="${pointer%\`\`}"; pointer="${pointer% }" ;;
        *) pointer="${pointer#\`}"; pointer="${pointer%\`}" ;;
    esac
    number="${pointer%% § *}"
    heading="${pointer#* § }"
    record="$(git ls-files -- "$DIR/$number-*.md")"
    if [ -z "$record" ]; then
        fail "$file:$line points at record $number, and there is no such record"
    elif ! grep -qxF -e "## $heading" -e "### $heading" -e "#### $heading" -- "$record"; then
        fail "$file:$line points at \"$heading\", and $record has no such heading"
    fi
done < <(git grep -noE '``[0-9]{3} § ([^`]|`[^`])+``|`[0-9]{3} § [^`]+`' -- "$DIR" ':(exclude)'"$DIR/README.md" || true)

verdict "$paths path(s) and $pointers pointer(s) resolve"
