#!/usr/bin/env bash
# The map of the tree, derived from it, so it is never stale: which crate
# or file holds what, in one call instead of a run of greps.
#
#   scripts/map.sh                  every crate: its files, its lines and
#                                   the description its Cargo.toml gives
#   scripts/map.sh <crate|dir>…     every source file under it (tests left
#                                   out): its lines and the first line of
#                                   its `//!` module doc
#
# A crate is named in full or without its prefix (`store` is
# `crates/lakelands-store`); anything else is a directory. The line count
# says which files to read by range, never whole. Only tracked files count.
# Written for bash 3.2 and later, the first shell a stranger has.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# The source files under a directory, unit tests (`tests.rs`) and
# integration tests (`tests/`) left out.
sources() {
    git ls-files -- "$1/*.rs" | grep -v -e '/tests\.rs$' -e '/tests/' || true
}

if [ $# -eq 0 ]; then
    for dir in crates/*/; do
        dir="${dir%/}"
        files="$(sources "$dir")"
        n=0 lines=0
        if [ -n "$files" ]; then
            n="$(printf '%s\n' "$files" | wc -l | tr -d ' ')"
            # shellcheck disable=SC2086 # paths in this tree hold no spaces
            lines="$(cat $files | wc -l | tr -d ' ')"
        fi
        desc="$(sed -n 's/^description *= *"\(.*\)"$/\1/p' "$dir/Cargo.toml" | head -1)"
        printf '%-24s %4s files %6s lines  %s\n' "${dir#crates/lakelands-}" "$n" "$lines" "${desc:-(no description)}"
    done
    echo "scripts/map.sh <crate> lists its files; fleet-where <name> finds a definition, where installed."
    exit 0
fi

for arg in "$@"; do
    dir="${arg%/}"
    [ -d "$dir" ] || dir="crates/lakelands-${arg#lakelands-}"
    if [ ! -d "$dir" ]; then
        echo "map: no crate or directory named $arg" >&2
        exit 1
    fi
    files="$(sources "$dir")"
    if [ -z "$files" ]; then
        echo "map: no tracked source file under $dir" >&2
        exit 1
    fi
    echo "$dir"
    printf '%s\n' "$files" | while IFS= read -r f; do
        head="$(grep -m1 '^//!' "$f" | sed 's|^//! *||' | cut -c1-120 || true)"
        printf '  %-44s %5s  %s\n' "${f#"$dir"/}" "$(wc -l < "$f" | tr -d ' ')" "${head:-(no module doc)}"
    done
done
