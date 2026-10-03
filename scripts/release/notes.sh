#!/usr/bin/env bash
# Writes the release notes for a tag: the titles of the pull requests
# merged since the previous tag, one per line. Titles are sentences in
# this repository, so the list reads as the changelog; there is no
# separate CHANGELOG file.
# Usage: scripts/release/notes.sh <tag>          (needs `gh` and GH_TOKEN)
#        scripts/release/notes.sh --pulls <tag>  (their numbers only; needs
#                                                 neither, for a rehearsal)
#
# The previous tag is the nearest earlier `v*` tag; a release skips
# earlier pre-releases, so its notes cover everything since the last
# release. With no previous tag, every merged pull request is listed.
#
# The pull requests of a release are the merges on the first-parent path
# from the previous tag to the new one: what was merged into the branch
# the release is cut from. A patch release is cut from `main`, which took
# the previous tag by a merge, so that tag is not on the path; its pull
# requests are then the merges no earlier release tag reaches. The plain
# range `previous..tag` would list an older patch line's pull requests
# with them, reached through that merge.
#
# A patch release's own pull requests are the steps of its release and
# say nothing of what it fixes. Its tag's message does, so the notes of a
# `vX.Y.Z` with Z above 0 begin with it: from the line `What ... :` to
# the paragraph `At the tag:`, that line as the heading.
#
# Below the list, "Changes a host must make" since the previous tag
# (scripts/release/host-changes.sh, which needs cargo-semver-checks). The
# notes are not written without it: a release that breaks a host must say
# how.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

only_pulls=false
[ "${1:-}" != "--pulls" ] || { only_pulls=true; shift; }
tag="${1:?usage: notes.sh [--pulls] <tag>}"

match='v*'
exclude=()
[[ "$tag" == *-* ]] || exclude=(--exclude 'v*-*')
previous="$(git describe --tags --abbrev=0 --match "$match" ${exclude[@]+"${exclude[@]}"} "$tag^" 2>/dev/null || true)"

# The numbers of the release's pull requests, oldest first.
pulls() {
    local revs=("$tag") earlier
    if [ -n "$previous" ] && git rev-list --first-parent "$tag" |
        grep -qx "$(git rev-parse "$previous^{commit}")"; then
        revs=(--first-parent "$previous..$tag")
    elif [ -n "$previous" ]; then
        revs+=(--not)
        while IFS= read -r earlier; do
            [ "$earlier" = "$tag" ] || { [ ${#exclude[@]} -gt 0 ] && [[ "$earlier" == *-* ]]; } ||
                revs+=("$earlier")
        done < <(git tag -l "$match" --merged "$tag")
    fi
    git log --merges --reverse --format=%s "${revs[@]}" |
        sed -n 's/^Merge pull request #\([0-9]*\) from .*/\1/p'
}
if $only_pulls; then
    pulls
    exit 0
fi
repo="${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"

if [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[1-9] ]]; then
    git tag -l --format='%(contents)' "$tag" | awk '
        /^At the tag:/ || /^Signed-off-by:/ { exit }
        !on && /^What .*:$/ { on = 1; sub(/:$/, ""); print "## " $0; print ""; next }
        on { print }'
fi

if [ -n "$previous" ]; then since="since $previous"; else since="to date"; fi
echo "Pull requests merged $since:"
echo
n=0
while read -r number; do
    title="$(gh api "repos/$repo/pulls/$number" --jq .title)"
    printf -- '- %s (%s#%s)\n' "$title" "${repo#*/}" "$number"
    n=$((n + 1))
done < <(pulls)
[ "$n" -gt 0 ] || echo "- No pull requests were merged."

echo
if [ -n "$previous" ]; then
    scripts/release/host-changes.sh "$previous" | sed '1s/^/## /; 1s/:$//'
else
    echo "## Changes a host must make"
    echo
    echo "- None to name: this is the first release."
fi
