#!/usr/bin/env bash
# Generates the third-party licence list that ships with a release.
# Usage: scripts/third-party-licences.sh [output]
#        (default: target/third-party-licences.txt)
#
# The output is a release artefact, never a tracked file. CI runs this
# script on every pull request so that it cannot rot between releases.
# It fails when a dependency's licence is not accepted, or when the
# accepted list in about.toml and the allow-list in deny.toml differ.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

out="${1:-target/third-party-licences.txt}"

# The quoted SPDX ids of the TOML array named $2 in file $1, sorted.
# Comments are stripped first: they may quote licence expressions.
licence_list() {
    sed 's/#.*//' "$1" \
        | perl -0777 -ne 'print "$1\n" while /^\s*\Q'"$2"'\E\s*=\s*\[(.*?)\]/msg' \
        | grep -o '"[^"]*"' | tr -d '"' | sort
}

deny="$(licence_list deny.toml allow)"
about="$(licence_list about.toml accepted)"
if [ -z "$deny" ] || [ "$deny" != "$about" ]; then
    echo "third-party-licences: deny.toml [licenses].allow and about.toml accepted differ:" >&2
    diff <(printf '%s\n' "$deny") <(printf '%s\n' "$about") >&2 || true
    exit 1
fi

mkdir -p "$(dirname "$out")"
cargo about generate --workspace --all-features --locked --fail \
    --config about.toml --output-file "$out" scripts/third-party-licences.hbs
echo "third-party-licences: wrote $out"
