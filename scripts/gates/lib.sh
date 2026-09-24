# Shared by the gate scripts. Source it; do not run it.
#
# Every gate runs from the repository root, reads only tracked files and
# prints one line per violation followed by a verdict. Exit status 0 is
# green, 1 is red, 2 is a gate that could not run.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

GATE_NAME="$(basename "$0" .sh)"
GATE_SELF="scripts/gates/$(basename "$0")"
gate_failures=0

fail() {
    printf '%s: %s\n' "$GATE_NAME" "$*" >&2
    gate_failures=$((gate_failures + 1))
}

die() {
    printf '%s: cannot run: %s\n' "$GATE_NAME" "$*" >&2
    exit 2
}

verdict() {
    if [ "$gate_failures" -gt 0 ]; then
        printf '%s: RED (%d violation(s))\n' "$GATE_NAME" "$gate_failures" >&2
        exit 1
    fi
    printf '%s: green%s\n' "$GATE_NAME" "${1:+ ($1)}"
}

# The commits a pull request adds: GATE_BASE..HEAD. CI sets GATE_BASE to
# the base branch's commit; locally it defaults to the merge base with
# origin/develop. With no usable base, only HEAD is checked.
gate_range() {
    local base="${GATE_BASE:-}"
    if [ -z "$base" ] || ! git rev-parse -q --verify "$base^{commit}" >/dev/null 2>&1; then
        base="$(git merge-base origin/develop HEAD 2>/dev/null || true)"
    fi
    if [ -n "$base" ] && git rev-parse -q --verify "$base^{commit}" >/dev/null 2>&1; then
        printf '%s..HEAD\n' "$base"
    else
        printf 'HEAD^!\n'
    fi
}

# Tracked files, NUL-separated, excluding this gate's own script.
tracked_files() {
    git ls-files -z -- "$@" | perl -0 -ne "print unless \$_ eq \"$GATE_SELF\\0\""
}
