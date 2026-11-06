#!/usr/bin/env bash
# Holds `scripts/release/notes.sh --pulls` to a throwaway repository: a
# tag, then two pull requests merged after it, one with GitHub's own
# subject (`Merge pull request #N from …`) and one with this repository's
# (`Merge pull request <repo>#N from …`). Both numbers are listed, oldest
# first. It needs neither `gh` nor a network.
# Usage: scripts/release/notes-test.sh
set -euo pipefail
script="$(git rev-parse --show-toplevel)/scripts/release/notes.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"
git init -q -b develop .
git config user.name test
git config user.email test@example.com
git config commit.gpgsign false
git config tag.gpgsign false
git commit -q --allow-empty -m base
git tag v0.1.0
merge() { # <branch> <subject>
    git switch -q -c "$1"
    git commit -q --allow-empty -m "work $1"
    git switch -q develop
    git merge -q --no-ff "$1" -m "$2"
}
merge one 'Merge pull request #7 from owner/one'
merge two 'Merge pull request lakelands#12 from owner/two'
git tag v0.2.0
got="$("$script" --pulls v0.2.0)"
if [ "$got" = "$(printf '7\n12')" ]; then
    echo "notes-test: both subject forms are read (7, 12)"
else
    echo "notes-test: expected 7 and 12, got:" >&2
    printf '%s\n' "$got" >&2
    exit 1
fi
