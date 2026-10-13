#!/usr/bin/env bash
# Runs every gate and reports each one, without stopping at the first red.
# Usage: scripts/gates/all.sh            (GATE_BASE=<rev> to set the range)
set -uo pipefail
cd "$(dirname "$0")"
status=0
for gate in no-external-platform-deps cfg-budget vocabulary single-catalog \
            seam-width core-size conformance-required dco no-external-decisions \
            doc-heads-its-item detail-sources refusals records-resolve vectors-kept \
            conflict-markers; do
    ./"$gate".sh || status=1
done
exit "$status"
