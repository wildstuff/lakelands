#!/usr/bin/env bash
# Gate 12 — refusals.
#
# A refusal's case is its code and its parameters, and its sentence is
# its template, the only one
# (`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`,
# extended). Red when `docs/refusals.md` is not what the catalog says;
# when two templates of a closed set can say the same sentence; when a
# code the last release tag shipped is gone without its row in
# `scripts/refusals.withdrawn`, renamed or lost a parameter; when a code
# with such a row is in the catalog; when a code the tag's page lists
# as withdrawn has no row any more, or that page's "Withdrawn" table is
# not the table the gate reads;
# when a case is built with a parameter made by `format!`; or when an
# `Unknown` is made anywhere but the wire's reader. The rules and how
# each is read are in `scripts/refusals.py`.
. "$(dirname "$0")/lib.sh"

command -v python3 >/dev/null || die "python3 is needed"
python3 scripts/refusals.py --check
