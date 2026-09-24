#!/usr/bin/env bash
# Checks that a tag may be released.
# Usage: scripts/release/check-tag.sh <tag>
#
# Red when
#   - the tag is not `vX.Y.Z` or a pre-release `vX.Y.Z-<suffix>`;
#   - X.Y.Z is not the workspace version in Cargo.toml;
#   - the tag is lightweight: a release tag is annotated;
#   - the tag message has no `Signed-off-by:` naming its tagger,
#     `Name <email>`, compared without regard to case (the rule
#     scripts/gates/dco.sh applies to commits);
#   - a release (no suffix) points at a commit that is not on develop or
#     main. A pre-release may be tagged anywhere, so a release can be
#     rehearsed from a branch.
# Prints `prerelease=true|false` for the workflow on success.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

tag="${1:?usage: check-tag.sh <tag>}"
failures=0
fail() { printf 'check-tag: %s: %s\n' "$tag" "$*" >&2; failures=$((failures + 1)); }

if [[ ! "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)(-[0-9A-Za-z.-]+)?$ ]]; then
    fail "not vX.Y.Z or vX.Y.Z-<suffix>"
    exit 1
fi
version="${BASH_REMATCH[1]}"
prerelease=false
[ -n "${BASH_REMATCH[2]}" ] && prerelease=true

workspace="$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' Cargo.toml)"
[ "$version" = "$workspace" ] || fail "version $version is not the workspace version $workspace"

if [ "$(git cat-file -t "refs/tags/$tag")" != tag ]; then
    fail "lightweight tag; make an annotated tag whose message ends with a Signed-off-by line"
else
    tagger="$(git for-each-ref --format='%(taggername) %(taggeremail)' "refs/tags/$tag")"
    if ! git for-each-ref --format='%(contents)' "refs/tags/$tag" \
            | sed -n 's/^Signed-off-by: *//p' | grep -q -i -x -F -- "$tagger"; then
        fail "no Signed-off-by: $tagger in the tag message"
    fi
fi

if [ "$prerelease" = false ]; then
    commit="$(git rev-parse "refs/tags/$tag^{commit}")"
    on=false
    for branch in develop main; do
        if git rev-parse -q --verify "refs/remotes/origin/$branch" >/dev/null \
                && git merge-base --is-ancestor "$commit" "refs/remotes/origin/$branch"; then
            on=true
        fi
    done
    [ "$on" = true ] || fail "a release is tagged on develop or main; this commit is on neither"
fi

[ "$failures" -eq 0 ] || exit 1
echo "prerelease=$prerelease"
