#!/usr/bin/env bash
# Gate 8 — dco.
#
# Every commit a pull request adds is signed off under the Developer
# Certificate of Origin (CONTRIBUTING.md). Red when a non-merge commit in
# the range has no `Signed-off-by:` trailer carrying its author's email.
# Merge commits are exempt: they carry no authored change.
. "$(dirname "$0")/lib.sh"

range="$(gate_range)"
n=0
while IFS= read -r commit; do
    n=$((n + 1))
    email="$(git log -1 --format='%ae' "$commit")"
    if ! git log -1 --format='%(trailers:key=Signed-off-by,valueonly)' "$commit" \
            | grep -q -i -F "<$email>"; then
        fail "commit $(git log -1 --format='%h %s' "$commit"): no Signed-off-by for <$email>"
    fi
done < <(git rev-list --no-merges "$range")

verdict "$n commit(s) in $range"
