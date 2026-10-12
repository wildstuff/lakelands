#!/usr/bin/env bash
# Gate 14 — the vectors of a shipped format are kept.
#
# `docs/log-format/vectors/<N>/` holds the test vectors of log format N.
# The highest number is the format this tree writes; every lower one was
# shipped by a release, which every later release reads, and is kept as
# that release's tag holds it
# (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
# `docs/log-format/vectors/kept` names each with its tag and its path
# there. Red when a directory below the highest has no line there; when a
# line's tag is not in the repository; and when a kept directory differs
# from the tag's tree: a file changed, gone or added. And red when a kept
# lake (`docs/log-format/lakes/<N>/`) has no line in `lakes/kept`, or its
# files are not the ones that line's digest names.
. "$(dirname "$0")/lib.sh"

DIR=docs/log-format/vectors
LIST="$DIR/kept"
[ -f "$LIST" ] || die "$LIST is missing"

formats=$(tracked_files | tr '\0' '\n' | sed -n "s|^$DIR/\([0-9][0-9]*\)/.*|\1|p" | sort -un)
[ -n "$formats" ] || die "no $DIR/<format>/ directory"
newest=$(printf '%s\n' "$formats" | tail -1)

kept=0
for n in $formats; do
    [ "$n" = "$newest" ] && continue
    line=$(grep -E "^$n " "$LIST" || true)
    [ -n "$line" ] || { fail "$DIR/$n/ is below the newest format ($newest) and $LIST names no tag for it"; continue; }
    read -r _ tag path <<<"$line"
    git rev-parse -q --verify "refs/tags/$tag" >/dev/null || { fail "$LIST names $tag for format $n, and there is no such tag"; continue; }
    want=$(git ls-tree -r --name-only "$tag" -- "$path" | sed "s|^$path/||" | sort)
    have=$(tracked_files | tr '\0' '\n' | sed -n "s|^$DIR/$n/||p" | sort)
    if [ "$want" != "$have" ]; then
        fail "$DIR/$n/ does not hold the files $tag holds under $path: $(diff <(echo "$want") <(echo "$have") | grep '^[<>]' | tr '\n' ' ')"
        continue
    fi
    while IFS= read -r f; do
        git show "$tag:$path/$f" | cmp -s - "$DIR/$n/$f" || fail "$DIR/$n/$f differs from $tag:$path/$f"
    done <<<"$want"
    kept=$((kept + 1))
done
# The lakes a release of each format made and later releases read
# (`docs/log-format/lakes/<N>/`) are never edited either: `lakes/kept`
# names each with a digest of its files, SHA-256 over the lines
# "<path in the directory> <blob id>" in byte order (`LC_ALL=C`, so no
# locale changes it), which this gate makes again.
LAKES=docs/log-format/lakes
LAKES_LIST="$LAKES/kept"
lakes=$(tracked_files | tr '\0' '\n' | sed -n "s|^$LAKES/\([0-9][0-9]*\)/.*|\1|p" | sort -un)
held=0
for n in $lakes; do
    want=$(grep -E "^$n " "$LAKES_LIST" 2>/dev/null | awk '{ print $2 }' || true)
    [ -n "$want" ] || { fail "$LAKES/$n/ is a kept lake and $LAKES_LIST names no digest for it"; continue; }
    have=$(git ls-files -s -- "$LAKES/$n" | awk -v d="$LAKES/$n/" '{ sub(d, "", $4); print $4 " " $2 }' | LC_ALL=C sort | sha256sum | cut -d' ' -f1)
    [ "$have" = "$want" ] || { fail "$LAKES/$n/ is not the lake $LAKES_LIST names: its digest is $have, the list says $want"; continue; }
    held=$((held + 1))
done
verdict "$kept kept format(s) as their tags hold them; $newest written; $held kept lake(s)"
