#!/usr/bin/env bash
# Gate 8 — dco.
#
# Every commit a pull request adds is signed off under the Developer
# Certificate of Origin (CONTRIBUTING.md). Red when a non-merge commit in
# the range has no `Signed-off-by:` trailer reading exactly its author,
# `Name <email>`, compared without regard to case. Merge commits are
# exempt: they carry no authored change. These are the rules of the DCO
# app that reports on every pull request, so the two never disagree.
. "$(dirname "$0")/lib.sh"

range="$(gate_range)"
n=0
while IFS= read -r commit; do
    n=$((n + 1))
    author="$(git log -1 --format='%an <%ae>' "$commit")"
    # grep reads to the end (no -q), so git is never cut off mid-write.
    if ! git log -1 --format='%(trailers:key=Signed-off-by,valueonly)' "$commit" \
            | grep -i -x -F -- "$author" >/dev/null; then
        fail "commit $(git log -1 --format='%h %s' "$commit"): no Signed-off-by: $author"
    fi
done < <(git rev-list --no-merges "$range")

verdict "$n commit(s) in $range"
