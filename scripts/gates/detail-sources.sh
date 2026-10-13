#!/usr/bin/env bash
# Gate 11 — detail sources.
#
# A failure's `Detail` is the only free text an error carries, and it
# reaches every caller the error is answered to. So it is made from a
# foreign library's words only where those words are known to quote no
# record's key or values and no path on the host: `Detail::io` and
# `Detail::json` scrub by their own rule anywhere, and `Detail::scrubbed`
# is called only in the files listed here, each with the reason its
# words are safe
# (`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`,
# extended). Red when a product file (under a crate's `src/`, tests and
# the conformance suite aside) names `scrubbed` outside a comment, by
# any path or alias, and is not listed, or a listed file no longer does;
# and when one outside the client reads a closed set (an `Error`, a
# `Situation`, a `Detail`, a case or a place of any set) from JSON by
# name, which would make free text of whatever the JSON says. (A read whose type is only inferred is not seen here; 015
# states that limit.)
. "$(dirname "$0")/lib.sh"

# file|why its words are safe
ALLOWED='
crates/lakelands-engine-pg/src/lib.rs|a server error as its severity, SQLSTATE and primary message, never DETAIL, HINT or WHERE
crates/lakelands-engine-pg/src/connect.rs|TLS and PEM setup errors, about certificates the location names; an error of the URL parser, which names a parameter of the URL the operator wrote, never its value
crates/lakelands-engine-sqlite/src/lib.rs|SQLite messages name a failed constraint by its columns or its check, never a row
crates/lakelands-engine-sqlite/src/handle.rs|a failed VACUUM, in SQLite words that name no row
crates/lakelands-store/src/exec.rs|only whether the work panicked or was cancelled, never what a panic said
crates/lakelands-core/src/export.rs|an export is verified for its operator, from its own files: the parser names the member it does not know
crates/lakelands-store/src/import.rs|the operator imports its own export: the parser names the member it does not know
crates/lakelands-backings/src/transport.rs|HTTP and TLS client errors, about the exchange and never its body
crates/lakelands-client/src/wire.rs|HTTP client errors, on the caller side of the door
crates/lakelands-client/src/feed.rs|HTTP client errors, on the caller side of the door
crates/lakelands-client/src/doors.rs|HTTP client errors, on the caller side of the door
'

listed() { printf '%s\n' "$ALLOWED" | awk -F'|' 'NF == 2 { print $1 }'; }

product=(':(exclude)**/tests.rs' ':(exclude)**/tests/**' ':(exclude)**/conformance/**')

calls=$(git grep -l -P '^(?!\s*//).*\bscrubbed\b' -- 'crates/*/src/**' \
    ':(exclude)crates/lakelands-api/src/error/**' "${product[@]}" || true)

reads=$(git grep -n -E 'from_(value|str|slice|reader)::<[^>]*\b(Error|Situation|Detail|Seam|Exchange|Flaw|Remote|Case|Place|Said|Refusal|Missing|NameProblem|NameSource)>' \
    -- 'crates/*/src/**' ':(exclude)crates/lakelands-client/src/**' "${product[@]}" || true)
while IFS= read -r line; do
    [ -n "$line" ] || continue
    fail "$line reads an error from JSON outside the client, which makes free text of it"
done <<< "$reads"

# Each list is read from a string, never a pipe: `grep -q` stops at its
# first match, and under `pipefail` the writer it leaves behind dies of a
# broken pipe, which read as a missing line once in CI.
names=$(listed)
while IFS= read -r file; do
    [ -n "$file" ] || continue
    grep -qx -- "$file" <<< "$names" || fail "$file scrubs foreign words and is not listed with why they are safe"
done <<< "$calls"

while IFS= read -r file; do
    grep -qx -- "$file" <<< "$calls" || fail "$file is listed and no longer scrubs foreign words; take it off the list"
done <<< "$names"

verdict "$(printf '%s\n' "$calls" | grep -c . || true) file(s) scrub foreign words"
