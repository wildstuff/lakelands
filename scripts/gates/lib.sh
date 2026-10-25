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

# The pull request's base commit, or nothing when only HEAD is checked.
gate_base() {
    local range
    range="$(gate_range)"
    [ "${range%..HEAD}" != "$range" ] && printf '%s\n' "${range%..HEAD}"
    return 0
}

# The number assigned as `NAME=<number>` in FILE on the base commit, or
# the number FILE holds whole when NAME is empty; nothing when there is
# no base or no such number.
base_number() {
    local file="$1" name="${2:-}" base
    base="$(gate_base)"
    [ -n "$base" ] || return 0
    if [ -n "$name" ]; then
        git show "$base:$file" 2>/dev/null | sed -n "/^$name=[0-9]/{s/^$name=\([0-9][0-9]*\).*/\1/p;q;}"
    else
        git show "$base:$file" 2>/dev/null | tr -d '[:space:]' | grep -x '[0-9][0-9]*' || true
    fi
}

# As base_number, but 0 when there is a base and FILE or NAME is not on
# it: a budget the pull request opens stood at 0 before it.
base_number_or_zero() {
    local n
    n="$(base_number "$@")"
    [ -z "$n" ] && [ -n "$(gate_base)" ] && n=0
    printf '%s\n' "$n"
}

# A budget's number goes down by itself and is raised only by a recorded
# decision (docs/decisions/047-a-ceiling-is-raised-by-a-recorded-rule.md):
# red when WHAT rose from PREV to NOW, unless this pull request adds a
# row to docs/budgets.md for BUDGET whose "to" is NOW.
# raised BUDGET WHAT PREV NOW
raised() {
    local budget="$1" what="$2" prev="$3" now="$4" base
    [[ "$prev" =~ ^[0-9]+$ ]] && [ "$now" -gt "$prev" ] || return 0
    base="$(gate_base)"
    if [ -n "$base" ] && git diff --no-color "$base...HEAD" -- docs/budgets.md \
            | perl -ne '
                BEGIN { ($budget, $now) = (shift, shift) }
                next unless /^\+\s*\|/;
                my @col = map { s/^\s+|\s+$//gr } split /\|/;
                my ($name, $change) = @col[2, 3];
                next unless defined $change && $change =~ /\xe2\x86\x92|->/;
                my ($to) = $change =~ /(?:\xe2\x86\x92|->)\s*(.*)$/;
                $to =~ s/\D//g;
                $found = 1 if lc $name eq lc $budget && $to eq $now;
                END { exit($found ? 0 : 1) }
            ' "$budget" "$now"; then
        return 0
    fi
    fail "$what rose from $prev to $now; a raise needs a row in docs/budgets.md for $budget whose \"to\" is $now"
}
