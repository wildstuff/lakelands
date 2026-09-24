#!/usr/bin/env bash
# Gate 1 — no-external-platform-deps.
#
# Lakelands works below the platform it was extracted from and never
# depends on it: the dependency direction is platform -> Lakelands only.
# Red when
#   - any Cargo.lock names a package of the originating platform — a
#     name in its crate namespace (below), or a source in its repository;
#   - any Cargo.toml reaches a path outside this repository.
. "$(dirname "$0")/lib.sh"

# The originating platform's crates: its whole `wild-` namespace plus the
# two crates outside it that hold the storage core.
platform_name='^(wild[-_].*|providers|providers[-_]core)$'
platform_source='github\.com[/:]wildstuff/the-wild'

locks=0
while IFS= read -r -d '' lock; do
    locks=$((locks + 1))
    while IFS= read -r line; do fail "$line"; done < <(perl -ne '
        BEGIN { ($re_name, $re_src, $file) = splice @ARGV, 0, 3 }
        $name = $1 if /^name = "([^"]+)"/;
        print "$file:$.: platform crate `$1` in the lock file\n"
            if /^name = "([^"]+)"/ && $1 =~ /$re_name/;
        print "$file:$.: `$name` is sourced from the platform repository\n"
            if /^source = "([^"]+)"/ && $1 =~ /$re_src/;
    ' "$platform_name" "$platform_source" "$lock" "$lock")
done < <(tracked_files 'Cargo.lock' '*/Cargo.lock')
[ "$locks" -gt 0 ] || fail "no Cargo.lock is tracked; the dependency graph cannot be checked"

root="$(pwd -P)"
while IFS= read -r -d '' manifest; do
    dir="$(dirname "$manifest")"
    while IFS= read -r hit; do
        lineno="${hit%%:*}"
        path="${hit#*:}"
        target="$(realpath -m "$dir/$path")"
        case "$target/" in
            "$root"/*) ;;
            *) fail "$manifest:$lineno: path \`$path\` leaves the repository" ;;
        esac
    done < <(perl -ne 'print "$.:$1\n" while /\bpath\s*=\s*"([^"]+)"/g' "$manifest")
done < <(tracked_files 'Cargo.toml' '*/Cargo.toml')

verdict "$locks lock file(s)"
