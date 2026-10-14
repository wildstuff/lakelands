#!/usr/bin/env bash
# Before a merge: whether pull request <n>'s `host-surface` judgement is
# still the judgement of the develop it would merge into
# (docs/decisions/056-the-host-surface-changes-by-one-release-of-deprecation.md).
# The job judges the pull request merged into develop as develop stood
# when its CI ran. It is stale when that run has no `host-surface` job at
# all (its merge ref predates the job; a re-run keeps that ref and its
# `ci.yml`), or when a merge into develop since touched the surface
# crates' sources or manifests, the list or the judge itself. A run still
# going cannot tell yet. Then the pull request is closed and
# reopened, or rebased, so its CI runs on today's develop.
# Usage: scripts/release/surface-fresh.sh <pull request number>
# Exits 0 when fresh, 1 when stale, 2 when it cannot tell.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
n="${1:?usage: surface-fresh.sh <pull request number>}"
git fetch -q origin develop || { echo "surface-fresh: cannot run: no fetch" >&2; exit 2; }

branch="$(gh pr view "$n" --json headRefName -q .headRefName)" ||
    { echo "surface-fresh: cannot run: no pull request $n" >&2; exit 2; }
run="$(gh run list --workflow ci --event pull_request --branch "$branch" --limit 1 \
    --json databaseId,createdAt,status -q '.[0] | "\(.databaseId) \(.createdAt) \(.status)"')"
[ -n "$run" ] || { echo "surface-fresh: pull request $n has no CI run"; exit 1; }
read -r id created status <<<"$run"
[ "$status" = completed ] ||
    { echo "surface-fresh: cannot tell: run $id of pull request $n is $status" >&2; exit 2; }
job="$(gh run view "$id" --json jobs -q '.jobs[] | select(.name == "host-surface") | .conclusion')"
[ "$job" = success ] ||
    { echo "surface-fresh: run $id of pull request $n has no green host-surface job (${job:-none}): close and reopen it"; exit 1; }
moved="$(git log -1 --first-parent --format=%cI origin/develop -- \
    'crates/lakelands-api/src' 'crates/lakelands-client/src' 'crates/lakelands-core/src' \
    'crates/lakelands-store/src' 'crates/lakelands-engine-*/src' 'crates/*/Cargo.toml' \
    scripts/release/surface-accepted scripts/release/surface-held.sh)"
if [[ "$(date -d "$created" +%s)" -lt "$(date -d "$moved" +%s)" ]]; then
    echo "surface-fresh: run $id of pull request $n ($created) predates develop's last surface change ($moved): close and reopen it"
    exit 1
fi
echo "surface-fresh: run $id of pull request $n judged today's surface"
