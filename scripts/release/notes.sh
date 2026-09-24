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
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

tag="${1:?usage: notes.sh <tag>}"
repo="${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"

match='v*'
exclude=()
[[ "$tag" == *-* ]] || exclude=(--exclude 'v*-*')
previous="$(git describe --tags --abbrev=0 --match "$match" "${exclude[@]}" "$tag^" 2>/dev/null || true)"
range="${previous:+$previous..}$tag"

echo "Pull requests merged ${previous:+since $previous}${previous:-to date}:"
echo
n=0
while read -r number; do
    title="$(gh api "repos/$repo/pulls/$number" --jq .title)"
    printf -- '- %s (%s#%s)\n' "$title" "${repo#*/}" "$number"
    n=$((n + 1))
done < <(git log --merges --reverse --format=%s "$range" \
    | sed -n 's/^Merge pull request #\([0-9]*\) from .*/\1/p')
[ "$n" -gt 0 ] || echo "- No pull requests were merged."
