#!/usr/bin/env bash
# Writes the release notes for a tag: the titles of the pull requests
# merged since the previous tag, one per line. Titles are sentences in
# this repository, so the list reads as the changelog; there is no
# separate CHANGELOG file.
# Usage: scripts/release/notes.sh <tag>      (needs `gh` and GH_TOKEN)
#
# The previous tag is the nearest earlier `v*` tag; a release skips
# earlier pre-releases, so its notes cover everything since the last
# release. With no previous tag, every merged pull request is listed.
#
# Below the list, "Changes a host must make" since the previous tag
# (scripts/release/host-changes.sh, which needs cargo-semver-checks). The
# notes are not written without it: a release that breaks a host must say
# how.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

tag="${1:?usage: notes.sh <tag>}"
repo="${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"

match='v*'
exclude=()
[[ "$tag" == *-* ]] || exclude=(--exclude 'v*-*')
previous="$(git describe --tags --abbrev=0 --match "$match" ${exclude[@]+"${exclude[@]}"} "$tag^" 2>/dev/null || true)"
range="${previous:+$previous..}$tag"

if [ -n "$previous" ]; then since="since $previous"; else since="to date"; fi
echo "Pull requests merged $since:"
echo
n=0
while read -r number; do
    title="$(gh api "repos/$repo/pulls/$number" --jq .title)"
    printf -- '- %s (%s#%s)\n' "$title" "${repo#*/}" "$number"
    n=$((n + 1))
done < <(git log --merges --reverse --format=%s "$range" \
    | sed -n 's/^Merge pull request #\([0-9]*\) from .*/\1/p')
[ "$n" -gt 0 ] || echo "- No pull requests were merged."

echo
if [ -n "$previous" ]; then
    scripts/release/host-changes.sh "$previous" | sed '1s/^/## /; 1s/:$//'
else
    echo "## Changes a host must make"
    echo
    echo "- None to name: this is the first release."
fi
