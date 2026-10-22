#!/usr/bin/env bash
# Gate 7 — conformance-required.
#
# The conformance suite is the specification, so it runs against every
# engine on every pull request. Red when
#   - an engine crate is missing from ROSTER, or ROSTER names an engine
#     with no crate, lists one twice, or uses a status other than
#     `required` / `deferred`;
#   - a required engine has no `tests/conformance.rs` entry point, or its
#     crate holds source beyond its crate doc and the entry point does not
#     call the suite's runner (`run_all(` or `run_checks(` outside a `//` comment) — an
#     imported engine is checked, never passed on zero tests;
#   - an engine that is required on the pull request's base commit is
#     deferred or gone (the roster only grows);
#   - the CI workflow does not run the suite from the roster.
. "$(dirname "$0")/lib.sh"

ROSTER=conformance/roster
WORKFLOW=.github/workflows/ci.yml

[ -f "$ROSTER" ] || die "$ROSTER is missing"

parse() { awk '!/^[[:space:]]*(#|$)/ { print $1, $2 }'; }

declare -A status=()
while read -r engine st; do
    [[ "$engine" =~ ^[a-z0-9]+$ ]] || { fail "$ROSTER: bad engine name \`$engine\`"; continue; }
    [ -z "${status[$engine]:-}" ] || fail "$ROSTER: \`$engine\` is listed twice"
    case "$st" in
        required|deferred) status[$engine]="$st" ;;
        *) fail "$ROSTER: \`$engine\` has status \`$st\`, expected required or deferred" ;;
    esac
    [ -f "crates/lakelands-engine-$engine/Cargo.toml" ] \
        || fail "$ROSTER: \`$engine\` has no crate crates/lakelands-engine-$engine"
done < <(parse < "$ROSTER")

while IFS= read -r manifest; do
    engine="${manifest#crates/lakelands-engine-}"
    engine="${engine%/Cargo.toml}"
    [ -n "${status[$engine]:-}" ] || fail "engine crate lakelands-engine-$engine is not on $ROSTER"
done < <(git ls-files 'crates/lakelands-engine-*/Cargo.toml')

required=0
for engine in "${!status[@]}"; do
    [ "${status[$engine]}" = required ] || continue
    required=$((required + 1))
    [ -f "crates/lakelands-engine-$engine/tests/conformance.rs" ] \
        || fail "required engine \`$engine\` has no crates/lakelands-engine-$engine/tests/conformance.rs"
    entry="crates/lakelands-engine-$engine/tests/conformance.rs"
    code="$(git ls-files -z -- "crates/lakelands-engine-$engine/src" | xargs -0 -r cat \
        | { grep -v -E '^[[:space:]]*(//|$)' || true; } | wc -l | tr -d ' ')"
    # A call, not a mention: `run_all(` or `run_checks(` outside a `//` comment. grep reads
    # to the end (no -q): stopping at the first match would cut sed off,
    # and under pipefail its broken pipe would fail the pipeline.
    if [ "$code" -gt 0 ] && [ -f "$entry" ] \
        && ! sed 's|//.*||' "$entry" | grep -E 'run_(all|checks)[[:space:]]*\(' >/dev/null; then
        fail "required engine \`$engine\` has $code line(s) of source, and $entry does not run the suite"
    fi
done
[ "$required" -gt 0 ] || fail "$ROSTER has no required engine"

range="$(gate_range)"
if [ "${range%..HEAD}" != "$range" ]; then
    while read -r engine st; do
        [ "$st" = required ] || continue
        [ "${status[$engine]:-}" = required ] \
            || fail "\`$engine\` is required on the base commit and ${status[$engine]:-missing} here; the roster only grows"
    done < <(git show "${range%..HEAD}:$ROSTER" 2>/dev/null | parse)
fi

[ -f "$WORKFLOW" ] || fail "$WORKFLOW is missing"
grep -q 'scripts/conformance.sh --list' "$WORKFLOW" \
    || fail "$WORKFLOW does not take its engines from the roster (scripts/conformance.sh --list)"
grep -q 'scripts/conformance.sh "\${{ matrix.engine }}"' "$WORKFLOW" \
    || fail "$WORKFLOW does not run scripts/conformance.sh per engine"

verdict "$required required engine(s): $(scripts/conformance.sh --list | tr '\n' ' ')"
